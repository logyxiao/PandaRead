import { useCallback, useEffect, useRef, useState } from "react";
import CodeMirror from "@uiw/react-codemirror";
import { EditorView } from "@codemirror/view";

interface EditorSelection {
  start: number;
  end: number;
  quote: string;
}

export function CodeMirrorEditor({value,onChange,onSelect}:{value:string;onChange:(value:string)=>Promise<boolean>|void;onSelect:(selection:EditorSelection)=>void}) {
  const [draft,setDraft]=useState(value);
  const timer=useRef<number|undefined>(undefined);
  const latest=useRef(value);
  const revision=useRef(0);
  const dirty=useRef(false);
  const saving=useRef(false);
  const rerun=useRef(false);
  const save=useRef(onChange);
  const view=useRef<EditorView|null>(null);

  useEffect(()=>{save.current=onChange},[onChange]);
  useEffect(()=>{
    if(dirty.current||saving.current)return;
    setDraft(value);
    latest.current=value;
  },[value]);

  const flush=useCallback(async()=>{
    window.clearTimeout(timer.current);
    if(saving.current)return;
    if(!dirty.current)return;
    saving.current=true;
    do {
      rerun.current=false;
      const text=latest.current;
      const savedRevision=revision.current;
      const ok=await save.current(text);
      const changed=savedRevision!==revision.current;
      if(ok&&!changed)dirty.current=false;
      if(ok&&changed)rerun.current=true;
    } while(rerun.current);
    saving.current=false;
  },[]);

  useEffect(()=>()=>{window.clearTimeout(timer.current);void flush()},[flush]);

  const update=(next:string)=>{
    setDraft(next);
    latest.current=next;
    revision.current+=1;
    dirty.current=true;
    window.clearTimeout(timer.current);
    timer.current=window.setTimeout(()=>void flush(),1500);
  };
  const select=()=>{
    const selection=view.current?.state.selection.main;
    if(!selection||selection.empty)return;
    const start=Math.min(selection.from,selection.to);
    const end=Math.max(selection.from,selection.to);
    onSelect({start,end,quote:view.current!.state.sliceDoc(start,end)});
  };

  return <div className="cm-wrap" onMouseUp={select}><CodeMirror value={draft} onChange={update} onCreateEditor={editor=>{view.current=editor}} extensions={[EditorView.lineWrapping]} basicSetup={{lineNumbers:false,foldGutter:false,highlightActiveLine:false,searchKeymap:true}}/></div>;
}
