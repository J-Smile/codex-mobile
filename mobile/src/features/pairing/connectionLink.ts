export function parseConnectionKey(value: string) {
  const input = value.trim();
  if (/^[A-Za-z0-9_-]{32,128}$/.test(input)) return input;
  let url: URL;
  try { url = new URL(input); } catch { throw new Error('这不是 Codex 连接二维码。'); }
  const key = new URLSearchParams(url.hash.slice(1)).get('key');
  if (!['https:', 'http:'].includes(url.protocol) || url.username || url.password
    || !key || !/^[A-Za-z0-9_-]{32,128}$/.test(key)) {
    throw new Error('这不是 Codex 连接二维码，请扫描桌面程序中的二维码。');
  }
  return key;
}
