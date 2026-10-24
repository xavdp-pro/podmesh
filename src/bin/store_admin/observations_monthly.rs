//! Private root-admin opt-in. No functional startup or released migration calls this module.
use super::Error;
use podmesh::store::{migrations, DurableStore, Engine, Row};
use serde_json::Value;

// MariaDB's hard partition limit, including legacy and future; not a retention policy.
const MAX_PARTITIONS: usize = 8192;
#[derive(Clone, Debug)]
pub(super) struct Plan { first: Month, count: usize }
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Month { year: i64, month: u32 }
impl Month {
    fn parse(s: &str) -> Result<Self, Error> {
        let b=s.as_bytes();
        if b.len()!=7 || b[4]!=b'-' || !b.iter().enumerate().all(|(i,c)|i==4 || c.is_ascii_digit()) {
            return Err("first_month must be exactly YYYY-MM".into());
        }
        let year=s[..4].parse()?; let month=s[5..].parse()?;
        if !(1970..=9998).contains(&year) || !(1..=12).contains(&month) {
            return Err("first_month must be between 1970-01 and 9998-12".into());
        }
        Ok(Self{year,month})
    }
    fn next(self) -> Self { if self.month==12 {Self{year:self.year+1,month:1}} else {Self{month:self.month+1,..self}} }
    fn name(self) -> String {format!("p{:04}{:02}",self.year,self.month)}
    fn epoch(self) -> i64 {
        fn years(y:i64)->i64 {let n=y-1;365*n+n/4-n/100+n/400}
        let leap=self.year%4==0 && (self.year%100!=0 || self.year%400==0);
        let lengths=[31,if leap{29}else{28},31,30,31,30,31,31,30,31,30,31];
        let days: i64=lengths[..(self.month-1) as usize].iter().sum();
        (years(self.year)-years(1970)+days)*86400
    }
}
impl Plan {
    pub(super) fn parse(v:&Value)->Result<Self,Error>{
        let o=v.as_object().ok_or("monthly plan must be an object")?;
        if o.len()!=2 || o.keys().any(|k|k!="first_month" && k!="months") {
            return Err("monthly plan accepts only first_month and months".into());
        }
        let first=Month::parse(v["first_month"].as_str().ok_or("first_month missing")?)?;
        let count=v["months"].as_u64().filter(|n|*n>0 && *n<=(MAX_PARTITIONS-2) as u64).ok_or("months exceeds MariaDB partition limit or is not positive")? as usize;
        // Every bound must fit the documented calendar; no local clock/TZ conversion.
        if first.year*12+i64::from(first.month-1)+count as i64 > 9999*12 {
            return Err("monthly plan ends after 9999-01".into());
        }
        Ok(Self{first,count})
    }
    fn finite(&self)->Vec<(String,i64)>{
        let mut out=vec![("p_legacy".into(),self.first.epoch())];
        let mut month=self.first;
        for _ in 0..self.count {let next=month.next();out.push((month.name(),next.epoch()));month=next;}
        out
    }
    fn monthly_definitions(&self)->String{
        self.finite().into_iter().skip(1).map(|(name,bound)|format!("PARTITION {name} VALUES LESS THAN ({bound})")).chain(["PARTITION p_future VALUES LESS THAN MAXVALUE".into()]).collect::<Vec<_>>().join(",\n")
    }
    fn create_sql(&self)->String{
        format!("CREATE TABLE observations (\n id BIGINT NOT NULL AUTO_INCREMENT,\n observed_at BIGINT NOT NULL,\n operation VARCHAR(64) NOT NULL,\n result LONGTEXT NOT NULL,\n PRIMARY KEY (id, observed_at)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4\nPARTITION BY RANGE (observed_at) (\nPARTITION p_legacy VALUES LESS THAN ({}),\n{}\n);",self.first.epoch(),self.monthly_definitions())
    }
    fn end(&self)->Month{let mut m=self.first;for _ in 0..self.count{m=m.next();}m}
}
fn mariadb(db:&dyn DurableStore)->Result<(),Error>{
    if db.engine()!=Engine::Mariadb {return Err("monthly observations require MariaDB".into());}Ok(())
}
/// Caller has already checked real/effective root, private socket credentials and offline sessions.
/// Never converts even an empty existing schema. MariaDB DDL autocommits: preserve partial state.
pub(super) fn prepare_empty(db:&mut dyn DurableStore,plan:&Plan)->Result<(),Error>{
    mariadb(db)?;
    if !db.tables()?.is_empty(){return Err("monthly constructor requires an entirely empty NODE database; no in-place conversion".into());}
    db.execute_batch(&plan.create_sql())?;
    // Immutable 0001 CREATE IF NOT EXISTS observes our table; all other migrations are unchanged.
    migrations::apply(db)?;
    migrations::verify_current_node(db)?;
    let current=layout(db)?;
    if current.first!=plan.first || current.count!=plan.count {return Err("monthly constructor postcondition differs".into());}
    Ok(())
}
const PARTITIONS:&str="SELECT PARTITION_NAME, PARTITION_METHOD, PARTITION_EXPRESSION, PARTITION_DESCRIPTION, SUBPARTITION_NAME FROM information_schema.PARTITIONS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME='observations' ORDER BY PARTITION_ORDINAL_POSITION";
const INDEXES:&str="SELECT INDEX_NAME, COLUMN_NAME, CAST(NON_UNIQUE AS SIGNED) FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=DATABASE() AND TABLE_NAME='observations' ORDER BY INDEX_NAME, SEQ_IN_INDEX";
fn decode_layout(rows:&[Row])->Result<Plan,Error>{
    if rows.len()<3 || rows.len()>MAX_PARTITIONS {return Err("observations is not a recognized monthly table".into());}
    let first_name=rows[1].text(0)?;
    let b=first_name.as_bytes();
    if b.len()!=7 || b[0]!=b'p' || !b[1..].iter().all(u8::is_ascii_digit) {return Err("monthly partition name is invalid".into());}
    let first=Month::parse(&format!("{}-{}",&first_name[1..5],&first_name[5..7]))?;
    let plan=Plan{first,count:rows.len()-2};
    let finite=plan.finite();
    for (i,row) in rows.iter().enumerate(){
        if row.text(1)?!="RANGE" || row.text(2)?.replace('`',"").trim()!="observed_at" || !matches!(row.value(4)?,podmesh::store::Value::Null) {
            return Err("unsupported partition expression/method/subpartition".into());
        }
        if i==rows.len()-1 {
            if row.text(0)?!="p_future" || row.text(3)?!="MAXVALUE" {return Err("monthly future catch-all missing".into());}
        } else if row.text(0)?!=finite[i].0 || row.text(3)?.parse::<i64>()?!=finite[i].1 {
            return Err("monthly partition sequence/bound differs from UTC contract".into());
        }
    }
    Ok(plan)
}
fn layout(db:&mut dyn DurableStore)->Result<Plan,Error>{
    let indexes=db.query(INDEXES,&[])?;
    if indexes.len()!=2 || indexes[0].text(0)?!="PRIMARY" || indexes[1].text(0)?!="PRIMARY"
        || indexes[0].text(1)?!="id" || indexes[1].text(1)?!="observed_at"
        || indexes[0].integer(2)?!=0 || indexes[1].integer(2)?!=0 {
        return Err("monthly observations primary key must be exactly (id, observed_at)".into());
    }
    decode_layout(&db.query(PARTITIONS,&[])?)
}
fn advance_sql(current:&Plan,next:&Plan)->Result<String,Error>{
    if current.end()!=next.first {return Err("maintenance must start at current future lower bound".into());}
    if current.count+next.count+2>MAX_PARTITIONS {return Err("maintenance would exceed MariaDB partition limit".into());}
    Ok(format!("ALTER TABLE observations REORGANIZE PARTITION p_future INTO (\n{}\n);",next.monthly_definitions()))
}
/// Explicit offline root action. Late rows stay in old ranges; future rows are redistributed
/// by MariaDB, including a populated MAXVALUE. Never drops or purges a partition or row.
pub(super) fn advance(db:&mut dyn DurableStore,next:&Plan)->Result<(),Error>{
    mariadb(db)?;
    migrations::verify_current_node(db)?;
    let current=layout(db)?;
    let sql=advance_sql(&current,next)?;
    db.execute_batch(&sql)?;
    let after=layout(db)?;
    if after.first!=current.first || after.count!=current.count+next.count {return Err("monthly maintenance postcondition differs; preserve state for inspection".into());}
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use podmesh::store::{self,Value as Stored};
    use std::sync::Arc;
    fn plan(first:&str,n:usize)->Plan{Plan::parse(&serde_json::json!({"first_month":first,"months":n})).unwrap()}
    fn rows(p:&Plan)->Vec<Row>{
        p.finite().into_iter().map(|(n,b)|(n,b.to_string())).chain([("p_future".into(),"MAXVALUE".into())]).map(|(n,b)|Row::new(Arc::new(vec![]),vec![Stored::from(n),Stored::from("RANGE"),Stored::from("`observed_at`"),Stored::from(b),Stored::Null])).collect()
    }
    #[test]
    fn utc_epoch_leap_century_and_year_boundaries(){
        assert_eq!(Month::parse("1970-01").unwrap().epoch(),0);
        assert_eq!(Month::parse("2026-10").unwrap().epoch(),1790812800);
        assert_eq!(Month::parse("2024-03").unwrap().epoch()-Month::parse("2024-02").unwrap().epoch(),29*86400);
        assert_eq!(Month::parse("2100-03").unwrap().epoch()-Month::parse("2100-02").unwrap().epoch(),28*86400);
        assert_eq!(plan("2026-12",1).end(),Month::parse("2027-01").unwrap());
    }
    #[test]
    fn boundary_late_and_future_rows_have_no_gap(){
        let p=plan("2026-10",2);let finite=p.finite();
        let bucket=|t:i64|finite.iter().find(|(_,end)|t<*end).map(|(n,_)|n.as_str()).unwrap_or("p_future");
        let oct=p.first.epoch();let nov=p.first.next().epoch();let dec=p.end().epoch();
        assert_eq!(bucket(i64::MIN),"p_legacy");assert_eq!(bucket(oct-1),"p_legacy");
        assert_eq!(bucket(oct),"p202610");assert_eq!(bucket(nov-1),"p202610");
        assert_eq!(bucket(nov),"p202611");assert_eq!(bucket(dec),"p_future");assert_eq!(bucket(i64::MAX),"p_future");
        // Arrival order is deliberately unrelated to observed_at: no runtime clock cut-off.
        assert_eq!([dec+10,oct+10,oct-10].map(bucket),["p_future","p202610","p_legacy"]);
    }
    #[test]
    fn strict_plan_and_catalog_refuse_incompatible_inputs(){
        for v in [serde_json::json!({"first_month":"2026-1","months":1}),serde_json::json!({"first_month":"2026-10","months":0}),serde_json::json!({"first_month":"2026-10","months":8191}),serde_json::json!({"first_month":"2026-10","months":1,"purge":true})]{assert!(Plan::parse(&v).is_err());}
        let p=plan("2026-10",2);assert_eq!(decode_layout(&rows(&p)).unwrap().count,2);
        let mut bad=rows(&p);bad.swap(1,2);assert!(decode_layout(&bad).is_err());
        assert!(advance_sql(&p,&plan("2026-11",1)).is_err());
        assert!(advance_sql(&p,&plan("2026-12",1)).unwrap().contains("REORGANIZE PARTITION p_future"));
    }
    struct Nonempty;
    impl DurableStore for Nonempty {
        fn engine(&self)->Engine{Engine::Mariadb}
        fn tables(&mut self)->store::Result<Vec<String>>{Ok(vec!["observations".into()])}
        fn execute(&mut self,_:&str,_:&[Stored])->store::Result<u64>{panic!("unexpected DML")}
        fn execute_batch(&mut self,_:&str)->store::Result<()>{panic!("constructor reached existing DDL")}
        fn query(&mut self,_:&str,_:&[Stored])->store::Result<Vec<Row>>{panic!("unexpected query")}
        fn transaction(&mut self)->store::Result<Box<dyn store::Transaction+'_>>{panic!("unexpected transaction")}
        fn integrity_check(&mut self)->store::Result<store::Integrity>{panic!("unexpected integrity")}
    }
    #[test]
    fn constructor_refuses_even_an_empty_preexisting_table_without_ddl(){assert!(prepare_empty(&mut Nonempty,&plan("2026-10",1)).is_err());}
}
