import {open} from '@tauri-apps/plugin-dialog';
import {videoOutputLocation} from './videoOutput';
import type {Capabilities} from './types';

export function VideoOutputLocation(props: {
  capabilities:Capabilities;jobId:string;matchId:number|null;module:'高光时刻'|'剧情剪辑';
  directory:string|null;onChange:(value:string|null)=>void|Promise<void>;
  disabled:boolean;native:boolean;onError:(error:string)=>void;
}) {
  async function choose() {
    try {
      const directory=await open({directory:true,multiple:false,title:'选择本次视频输出目录',defaultPath:props.directory??props.capabilities.videoOutputDirectory??undefined});
      if(typeof directory==='string') await props.onChange(directory);
    } catch(error) {props.onError(String(error));}
  }
  return <div className="video-output-location" role="group" aria-label="视频输出位置">
    <strong className="video-output-title">视频输出位置</strong>
    <p className="video-output-path">{videoOutputLocation(props.capabilities,props.jobId,props.matchId,props.module,props.directory)}</p>
    <small className="video-output-note">{props.directory?'本次使用自定义目录。':'使用全局默认目录，可在顶部设置中更改。'}</small>
    <div className="video-output-actions">
      <button className="secondary-button video-output-button" type="button" disabled={props.disabled||!props.native} onClick={()=>void choose()}>选择输出位置</button>
      <button className="secondary-button video-output-button" type="button" disabled={props.disabled||!props.directory} onClick={()=>void props.onChange(null)}>使用默认位置</button>
    </div>
  </div>;
}
