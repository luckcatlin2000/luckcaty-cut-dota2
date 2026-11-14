import { useId } from "react";
import type { ClipCameraMode } from "./types";
import "./cameraModeDiagram.css";

export function CameraModeDiagram({ mode }: { mode: ClipCameraMode }) {
  const id = useId();
  const following = mode === "hero_chase";
  const frameX = following ? 82 : 32;
  const centerX = frameX + 95;
  const heroX = following ? centerX : 197;
  const heroY = following ? 60 : 67;
  const title = following ? "英雄跟随机位示意" : "玩家视角机位示意";
  const description = following
    ? "镜头随当前英雄移动，画面范围也随之移动。"
    : "还原玩家当时观察的区域，英雄可以偏离画面中央。";

  return (
    <figure
      className="camera-mode-diagram"
      data-camera-mode={following ? "hero_chase" : "player_perspective"}
    >
      <div className="camera-mode-diagram-heading">
        <span>机位示意</span>
        <small>非游戏实录</small>
      </div>
      <svg
        viewBox="0 0 320 126"
        role="img"
        aria-labelledby={`${id}-title ${id}-description`}
      >
        <title id={`${id}-title`}>{title}</title>
        <desc id={`${id}-description`}>{description}</desc>
        <path
          className="camera-diagram-ground"
          d="M15 44H305M15 76H305M64 16V99M128 16V99M192 16V99M256 16V99"
        />
        {following && (
          <>
            <rect className="camera-diagram-previous" x="28" y="24" width="190" height="72" rx="8" />
            <path className="camera-diagram-motion" d="M57 82Q116 61 159 60" />
            <path className="camera-diagram-arrow" d="M153 55L165 60L153 65Z" />
          </>
        )}
        <rect className="camera-diagram-frame" x={frameX} y="24" width="190" height="72" rx="8" />
        <text className="camera-diagram-frame-label" x={frameX + 9} y="39">当前画面范围</text>
        <path
          className="camera-diagram-center"
          d={`M${centerX - 7} 60H${centerX + 7}M${centerX} 53V67`}
        />
        {following && <circle className="camera-diagram-center" cx={centerX} cy="60" r="13" />}
        <circle className="camera-diagram-hero-halo" cx={heroX} cy={heroY} r="11" />
        <circle className="camera-diagram-hero" cx={heroX} cy={heroY} r="6" />
        <text className="camera-diagram-hero-label" x={heroX + 15} y={heroY + 4}>英雄</text>
        {following && (
          <>
            <path className="camera-diagram-follow" d="M278 60H302M295 54L302 60L295 66" />
            <text className="camera-diagram-follow-label" x="246" y="15">镜头跟随</text>
          </>
        )}
        <circle className="camera-diagram-hero" cx="39" cy="115" r="4" />
        <text x="49" y="119">英雄</text>
        <path className="camera-diagram-center" d="M111 115H123M117 109V121" />
        <text x="130" y="119">镜头中心</text>
        <rect className="camera-diagram-legend-frame" x="230" y="110" width="14" height="10" rx="2" />
        <text x="251" y="119">画面范围</text>
      </svg>
      <figcaption>{description}</figcaption>
    </figure>
  );
}
