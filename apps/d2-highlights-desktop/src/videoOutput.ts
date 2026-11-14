import type {Capabilities} from './types.ts';

export function videoOutputLocation(capabilities: Capabilities, jobId: string, matchId: number|null, module: '高光时刻'|'剧情剪辑', override: string|null = null): string {
  const root = override ?? capabilities.videoOutputDirectory;
  const path = root
    ? `${root.replace(/[\\/]$/, '')}\\${matchId ?? jobId}\\${module}`
    : `${capabilities.jobsRoot || '任务目录'}\\${jobId}\\output`;
  return path.replace(/^\\\\\?\\/, '');
}
