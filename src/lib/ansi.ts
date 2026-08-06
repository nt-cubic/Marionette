/** Strip CSI/OSC and other common ANSI sequences for Clean View. */
export function stripAnsi(input: string): string {
  return input
    // OSC ... BEL or ST
    .replace(/\u001b\][^\u0007\u001b]*(?:\u0007|\u001b\\)/g, "")
    // CSI sequences
    .replace(/\u001b\[[0-9;?]*[ -/]*[@-~]/g, "")
    // charset / misc single-char escapes
    .replace(/\u001b[()][0-9A-Za-z]/g, "")
    .replace(/\u001b[=>]/g, "")
    // remaining ESC + char
    .replace(/\u001b./g, "")
    // Orphan SGR after ESC was lost (PowerShell / copy-paste leaves `[32;1m`)
    .replace(/\[(?:\d{1,3};){0,8}\d{1,3}m/g, "")
    // strip other C0 controls except tab/newline
    .replace(/[\u0000-\u0008\u000B\u000C\u000E-\u001F]/g, "");
}

/** Normalize newlines after ANSI strip for transcript cards. */
export function ansiToPlainText(input: string): string {
  return stripAnsi(input)
    .replace(/\r\n/g, "\n")
    .replace(/\r/g, "\n")
    // collapse huge runs of blank lines from full-screen redraws
    .replace(/\n{4,}/g, "\n\n");
}
