import moduleCatalog from "../module-catalog.json" with { type: "json" };

export interface ExternalToolAction {
  protocol: "local" | "mcp" | "api";
  operation: string;
}

export interface ExternalToolDescriptor {
  id: string;
  name: string;
  availability: "planned" | "download_required";
  action?: ExternalToolAction;
}

export type ExternalToolId = string;

export const UNRELEASED_MODULE_MESSAGE = "尚未正式提供，当前版本仅提供高光时刻；后续模块将独立发布，提供并安装后可从这里打开。";

// The shared release catalog controls availability independently of local installation.
export const EXTERNAL_TOOLS: readonly ExternalToolDescriptor[] = moduleCatalog.modules.map(tool => ({
  id: tool.id,
  name: tool.name,
  availability: tool.released ? "download_required" : "planned",
}));

export function canLaunchExternalTool(id: ExternalToolId, tools: readonly ExternalToolDescriptor[] = EXTERNAL_TOOLS): boolean {
  return tools.some(tool => tool.id === id && tool.availability === "download_required");
}

export function externalToolNotice(
  id: ExternalToolId,
  tools: readonly ExternalToolDescriptor[] = EXTERNAL_TOOLS,
): { title: string; message: string } {
  const tool = tools.find((entry) => entry.id === id);
  if (!tool) {
    return { title: "外接模块尚未正式提供", message: UNRELEASED_MODULE_MESSAGE };
  }
  return {
    title: tool.availability === "planned" ? `${tool.name}尚未正式提供` : `${tool.name}需单独下载`,
    message: tool.availability === "planned"
      ? UNRELEASED_MODULE_MESSAGE
      : "未检测到这款独立软件。请单独下载并安装后重试。",
  };
}
