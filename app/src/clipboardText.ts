// Copy plain text to the system clipboard.
//
// `navigator.clipboard.writeText` is the path that works: Tauri 2 exposes the
// browser API in WebView2, and WebView2 grants clipboard WRITE (it denies READ
// — that is why reading goes through the Rust `readClipboardText` command
// instead, see App.tsx). Older WebView2 builds don't grant even write, so the
// off-screen <textarea> + execCommand fallback stays.
//
// FileManagerPane.copyPathOf still carries its own copy of this pair; folding
// it onto this helper is logged in BACKLOG rather than done in passing.

export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const ta = document.createElement("textarea");
    ta.value = text;
    ta.style.position = "fixed";
    ta.style.opacity = "0";
    document.body.appendChild(ta);
    ta.select();
    try {
      return document.execCommand("copy");
    } catch {
      return false;
    } finally {
      document.body.removeChild(ta);
    }
  }
}
