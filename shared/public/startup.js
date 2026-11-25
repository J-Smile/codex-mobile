const savedTheme = localStorage.getItem('codex-mobile-theme');
const theme = savedTheme === 'light' || savedTheme === 'dark' ? savedTheme : matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light';
document.documentElement.dataset.theme = theme;
document.querySelector('meta[name="theme-color"]').content = theme === 'dark' ? '#1c1c1e' : '#ffffff';
document.querySelector('meta[name="color-scheme"]').content = theme;
async function reloadCodexPage() {
  const assets = document.querySelectorAll('script[type="module"][src], link[rel="modulepreload"], link[rel="stylesheet"]');
  await Promise.all([...assets].map(async asset => {
    const response = await fetch(asset.src || asset.href, { cache: 'reload' });
    if (!response.ok) throw new Error('页面资源暂时无法读取');
    await response.arrayBuffer();
  }));
  location.reload();
}
function showStartupFailure() {
  const status = document.getElementById('startup-status');
  const retry = document.getElementById('startup-retry');
  if (!status || !retry) return;
  status.textContent = '页面未能打开，请检查网络后重试';
  status.setAttribute('role', 'alert');
  retry.hidden = false;
  retry.onclick = async () => {
    retry.disabled = true;
    status.textContent = '正在重新打开…';
    try { await reloadCodexPage(); }
    catch {
      status.textContent = '网络仍不可用，请稍后重试';
      retry.disabled = false;
    }
  };
}
addEventListener('codex-startup-failed', showStartupFailure);
addEventListener('error', event => {
  if (event.target instanceof HTMLScriptElement && event.target.type === 'module') showStartupFailure();
}, true);
