// The replication panel against a running console on the lab (not a fixture): open a universe's drawer,
// target one standby, replicate now, arm the schedule every minute, wait for a scheduled run, disarm it,
// and read the Health view's replication column. Needs CONSOLE_URL and UNIVERSE; screenshots go to SHOTS.
import {chromium} from '../../node_modules/playwright/index.mjs';
const url=process.env.CONSOLE_URL,U=process.env.UNIVERSE,shots=process.env.SHOTS||'.';
const browser=await chromium.launch();const page=await browser.newPage({viewport:{width:1400,height:1000}});
const step=async(name,fn)=>{const t=Date.now();try{await fn();console.log(`ok ${name} (${((Date.now()-t)/1000).toFixed(1)} s)`);}catch(e){console.log(`FAIL ${name}: ${e.message.split('\n')[0]}`);console.log((await page.locator('body').innerText()).slice(0,2500));await page.screenshot({path:`${shots}/replication-fail.png`,fullPage:true});await browser.close();process.exit(1);}};
let drawer,panel;
await step('open the universe drawer',async()=>{await page.goto(url+'/');await page.getByRole('button',{name:/Universes/}).first().click();
 await page.getByRole('button',{name:'podmesh-'+U,exact:true}).first().click({timeout:30000});drawer=page.getByRole('dialog',{name:'Container details'});panel=drawer.locator('.replication');
 await panel.getByText('Replicate to').waitFor({timeout:60000});});
await step('target one standby, every minute, and save',async()=>{
 await panel.getByRole('button',{name:'Replicate to',exact:true}).click();await page.getByRole('option',{name:/^1 host/}).click();
 await panel.getByRole('button',{name:'Schedule',exact:true}).click();await page.getByRole('option',{name:/every minute/}).click();
 await panel.getByPlaceholder('Approved task or mandate').fill('console-replication-check');
 await panel.getByRole('button',{name:/Save target|Set up replication/}).click();
 await panel.getByText(/Target: 1 standby host/).waitFor({timeout:120000});});
await step('replicate now',async()=>{await panel.getByRole('button',{name:'Replicate now'}).click();await panel.getByRole('button',{name:'Replicating…'}).waitFor({timeout:5000});
 await panel.getByRole('button',{name:'Replicate now'}).waitFor({timeout:300000});await panel.getByText(/Last run .*: ok\./).waitFor({timeout:60000});
 await page.screenshot({path:`${shots}/replication-drawer.png`});});
await step('start the schedule and see a scheduled run',async()=>{const before=await panel.innerText();await panel.getByRole('button',{name:'Start schedule'}).click();
 await panel.getByText('scheduled',{exact:true}).waitFor({timeout:60000});
 await page.waitForTimeout(100000);await panel.getByRole('button',{name:'Refresh replication'}).click();await page.waitForTimeout(8000);
 const after=await panel.innerText();console.log('  before:',before.match(/Last run[^.]*\./)?.[0],'| after:',after.match(/Last run[^.]*\./)?.[0]);});
await step('stop the schedule',async()=>{await panel.getByRole('button',{name:'Stop schedule'}).click();await panel.getByText('not scheduled',{exact:true}).waitFor({timeout:60000});
 await page.screenshot({path:`${shots}/replication-stopped.png`});});
await step('health shows the replication column',async()=>{await drawer.getByRole('button',{name:'Close details'}).click();await page.getByRole('button',{name:'Health'}).click();
 await page.getByRole('columnheader',{name:'Replication'}).waitFor({timeout:90000});await page.getByText(/1 standby · last copy/).waitFor({timeout:90000});
 await page.screenshot({path:`${shots}/replication-health.png`,fullPage:true});});
await browser.close();
