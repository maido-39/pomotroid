// Helpers for handing user-picked files to Rust on Android.
//
// The dialog plugin returns `content://` URIs on Android that `std::fs` cannot
// open, so the webview reads the bytes via tauri-plugin-fs and passes them to
// the `*_bytes` commands as base64.

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = '';
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

/** Best-effort display name from a path or content:// URI. */
export function displayNameFromPath(path: string): string {
  const tail = path.split(/[\\/]/).pop() ?? 'file';
  try {
    return decodeURIComponent(tail);
  } catch {
    return tail;
  }
}

export async function readPickedFileAsBase64(
  path: string
): Promise<{ name: string; dataB64: string }> {
  const { readFile } = await import('@tauri-apps/plugin-fs');
  const bytes = await readFile(path);
  return { name: displayNameFromPath(path), dataB64: bytesToBase64(bytes) };
}
