import type { ReplayLibrarySnapshot } from "./types";

export function replayLibraryDevelopmentSnapshot(directory: string): ReplayLibrarySnapshot {
  const remembered = localStorage.getItem("cat-cut-dev-replay-library");
  if (remembered) return JSON.parse(remembered) as ReplayLibrarySnapshot;
  const root = directory || "D:\\Dota\\game\\dota\\replays";
  return {
    directory: root,
    files: [
      { name: "1000000002.dem", path: `${root}\\1000000002.dem`, byteLength: 64000000, downloadedAtUnixMs: Date.UTC(2026, 9, 30, 8, 0), timeBasis: "created" },
      { name: "1000000001.dem", path: `${root}\\1000000001.dem`, byteLength: 48000000, downloadedAtUnixMs: Date.UTC(2026, 9, 29, 8, 0), timeBasis: "created" },
    ],
  };
}
