export function preferredAnalysisExportDirectory(
  rememberedDirectory: string,
  recommendedDirectory: string | null,
) {
  return rememberedDirectory.trim() || recommendedDirectory?.trim() || undefined;
}
