export async function copyText(text: string) {
  if (navigator.clipboard && window.isSecureContext) {
    await navigator.clipboard.writeText(text);
    return;
  }
  // 局域网 HTTP 页面不能使用 Clipboard API，保留手机上的复制操作。
  const previous = document.activeElement as HTMLElement | null;
  const input = document.createElement('textarea');
  input.value = text;
  input.setAttribute('readonly', '');
  input.style.position = 'fixed';
  input.style.opacity = '0';
  document.body.append(input);
  input.select();
  const copied = document.execCommand('copy');
  input.remove();
  previous?.focus();
  if (!copied) throw new Error('复制失败，请长按链接文本复制。');
}
