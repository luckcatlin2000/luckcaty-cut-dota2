import assert from 'node:assert/strict';
import test from 'node:test';
import {videoOutputLocation} from './videoOutput.ts';
import type {Capabilities} from './types.ts';
const caps={jobsRoot:'C:\\jobs',videoOutputDirectory:null} as Capabilities;
test('both modules keep the original highlight default, and share global custom roots',()=>{
  assert.equal(videoOutputLocation(caps,'d2h-test',123,'高光时刻'),'C:\\jobs\\d2h-test\\output');
  assert.equal(videoOutputLocation(caps,'d2h-test',123,'剧情剪辑'),videoOutputLocation(caps,'d2h-test',123,'高光时刻'));
  const custom={...caps,videoOutputDirectory:'D:\\视频'};
  assert.equal(videoOutputLocation(custom,'d2h-test',123,'剧情剪辑'),'D:\\视频\\123\\剧情剪辑');
  assert.equal(videoOutputLocation(custom,'d2h-test',123,'高光时刻'),'D:\\视频\\123\\高光时刻');
  assert.equal(videoOutputLocation(custom,'d2h-test',null,'剧情剪辑','\\\\?\\E:\\作品'),'E:\\作品\\d2h-test\\剧情剪辑');
  assert.equal(videoOutputLocation(custom,'d2h-test',123,'剧情剪辑',null),'D:\\视频\\123\\剧情剪辑');
});
