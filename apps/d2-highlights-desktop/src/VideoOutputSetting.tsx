import {useEffect,useState} from 'react';
import {invoke} from '@tauri-apps/api/core';
import {open} from '@tauri-apps/plugin-dialog';
import {FolderOpen} from 'lucide-react';
import type {Capabilities} from './types';

export function VideoOutputSetting({capabilities,blocked,native,onChange}:{capabilities:Capabilities;blocked:boolean;native:boolean;onChange:(value:Capabilities)=>void}) {
  const [draft,setDraft]=useState(capabilities.videoOutputDirectory??''),[busy,setBusy]=useState(false),[error,setError]=useState(''),[notice,setNotice]=useState('');
  useEffect(()=>setDraft(capabilities.videoOutputDirectory??''),[capabilities.videoOutputDirectory]);
  async function save(path:string|null) {
    setBusy(true);setError('');setNotice('');
    try {
      if(path!==null&&!path.trim())throw new Error('请输入输出文件夹路径，或选择恢复默认。');
      let updated:Capabilities;
      if(native) updated=await invoke<Capabilities>('set_video_output_directory',{path:path?.trim()??null});
      else if(import.meta.env.DEV) {
        localStorage.setItem('cat-cut-dev-video-output',JSON.stringify(path));
        updated={...capabilities,videoOutputDirectory:path};
      } else throw new Error('请在桌面软件中设置输出位置。');
      onChange(updated);setNotice('已保存，高光和剧情共用此默认设置。');
    } catch(e) {setError(String(e));} finally {setBusy(false);}
  }
  async function choose() {
    try {const selected=await open({directory:true,multiple:false,title:'选择默认视频输出目录',defaultPath:draft||undefined});
      if(typeof selected==='string')await save(selected);
    } catch(e) {setError(String(e));}
  }
  return <div className="dota-path-setting" aria-label="全局视频输出设置">
    <div className="dota-path-heading"><strong>默认视频输出目录</strong><small>{capabilities.videoOutputDirectory?'自定义':'自动'}</small></div>
    <small className="dota-path-help">高光与剧情共用。自动使用本局原高光输出目录；自定义目录按录像编号与模块归档。</small>
    <div className="dota-path-input-row"><input aria-label="默认视频输出目录" value={draft} placeholder="自动：本局任务的 output 文件夹" disabled={blocked||busy} onChange={e=>{setDraft(e.target.value);setError('');setNotice('')}}/>
    <button aria-label="选择默认视频输出目录" title="选择默认视频输出目录" disabled={blocked||busy||!native} onClick={()=>void choose()}><FolderOpen/></button></div>
    <div className="dota-path-actions"><button disabled={blocked||busy} onClick={()=>void save(draft)}>保存输出目录</button><button disabled={blocked||busy||!capabilities.videoOutputDirectory} onClick={()=>void save(null)}>恢复默认输出目录</button></div>
    {error&&<p className="dota-path-error" role="alert">{error}</p>}{notice&&<p className="dota-path-help" role="status">{notice}</p>}
  </div>;
}
