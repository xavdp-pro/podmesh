import {useEffect,useRef,useState} from 'react';import {ChevronDown,X} from 'lucide-react';
// A styled list, never the browser's: keyboard-driven; past four options, or when asked, a field that filters and a cross that empties it.
export default function Select({value,onChange,options,placeholder='—',className='',searchable=false,searchPlaceholder='Search…',noResult='No match',label,disabled=false}){
 const[open,O]=useState(false),[focused,F]=useState(-1),[query,Q]=useState('');const box=useRef(null),list=useRef(null);
 const canSearch=searchable||options.length>4,selected=options.find(o=>o.value===value),selectedIndex=options.findIndex(o=>o.value===value);
 const visible=canSearch&&query.trim()?options.filter(o=>(o.label+' '+(o.hint||'')).toLowerCase().includes(query.trim().toLowerCase())):options;
 useEffect(()=>{const onDown=e=>{if(box.current&&!box.current.contains(e.target))O(false);};document.addEventListener('mousedown',onDown);return()=>document.removeEventListener('mousedown',onDown);},[]);
 useEffect(()=>{if(open&&list.current&&focused>=0)list.current.children[focused]?.scrollIntoView({block:'nearest'});},[focused,open]);
 const toggle=()=>{if(disabled)return;O(v=>{if(!v){F(selectedIndex>=0?selectedIndex:0);if(canSearch)Q('');}return!v;});};
 const pick=o=>{onChange(o.value);O(false);};
 const onKeyDown=e=>{if(!open){if(['Enter',' ','ArrowDown'].includes(e.key)){e.preventDefault();toggle();}return;}
  if(e.key==='ArrowDown'){e.preventDefault();F(i=>Math.min(i+1,visible.length-1));}else if(e.key==='ArrowUp'){e.preventDefault();F(i=>Math.max(i-1,0));}
  else if(e.key==='Enter'||e.key===' '){e.preventDefault();if(focused>=0&&visible[focused])pick(visible[focused]);}else if(e.key==='Escape'){e.preventDefault();O(false);}};
 return <div ref={box} className={'pm-select '+className}>
  <button type="button" onClick={toggle} onKeyDown={onKeyDown} aria-haspopup="listbox" aria-expanded={open} aria-label={label} disabled={disabled} className="pm-select-button">
   <span className={selected?'':'placeholder'}>{selected?selected.label:placeholder}</span><ChevronDown size={15} className={open?'rotated':''}/></button>
  {open&&<div className="pm-select-pop">
   {canSearch&&<div className="pm-select-search"><input value={query} onChange={e=>{Q(e.target.value);F(0);}} onKeyDown={onKeyDown} placeholder={searchPlaceholder} autoFocus aria-label={searchPlaceholder}/>
    {query&&<button type="button" onClick={()=>{Q('');F(0);}} aria-label="Clear the search"><X size={14}/></button>}</div>}
   <ul ref={list} role="listbox">{visible.length===0?<li className="pm-select-empty">{noResult}</li>:visible.map((o,i)=><li key={o.value} role="option" aria-selected={o.value===value} onMouseEnter={()=>F(i)} onMouseDown={()=>pick(o)} className={(i===focused?'focused ':'')+(o.value===value?'selected':'')}>{o.label}{o.hint&&<small>{o.hint}</small>}</li>)}</ul>
  </div>}
 </div>;
}
