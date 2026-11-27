import QRCode from 'qrcode';
import { request, type Status, type RelayPairing, type Config } from './api';
import './style.css';

const savedTheme = localStorage.getItem('codex-mobile-desktop-theme');
const systemTheme = matchMedia('(prefers-color-scheme: dark)');
document.documentElement.dataset.theme = savedTheme || (systemTheme.matches ? 'dark' : 'light');
document.querySelector('#app')!.innerHTML = `
  <header class="app-bar">
    <div class="brand"><img src="/icon.svg" width="28" height="28" alt="">Codex Mobile</div>
    <button id="open-settings" class="secondary-button" type="button" aria-haspopup="dialog">设置</button>
  </header>
  <main class="dashboard">
    <section id="relay-access" class="relay-access" aria-labelledby="connect-title" data-feedback>
      <div class="connect-copy">
        <h1 id="connect-title">连接手机</h1>
        <p class="connect-description">在手机网页点「添加电脑」，扫描这里的二维码。</p>
        <p id="device-name" class="device-name" hidden><span class="monitor-symbol" aria-hidden="true"></span><span id="device-label"></span></p>
        <div class="connect-actions"><button id="link-phone" class="primary-button" type="button" disabled>手机扫码连接</button>
          <button id="copy-relay" class="quiet-button" type="button" hidden>复制连接链接</button></div>
        <p id="invite-hint" class="hint" hidden>手机网页 <strong id="mobile-address"></strong></p>
        <p id="connect-hint" class="hint" hidden>生成二维码时会启动连接服务。</p>
      </div>
      <div class="connection-stage">
        <div id="pair-art" class="pair-art" aria-hidden="true">
          <div class="diagram-computer"><div class="diagram-window"><span></span><span></span><span></span></div><div class="diagram-screen"><b>Codex</b><i></i><i></i></div></div>
          <div class="diagram-phone"><div class="diagram-speaker"></div><span class="diagram-scan"></span></div>
        </div>
        <div id="relay-invite" class="relay-invite" hidden><canvas id="relay-qr" width="240" height="240" role="img" aria-label="电脑连接二维码"></canvas></div>
      </div>
      <p id="notice" class="notice" role="status" hidden></p>
    </section>
    <div class="dashboard-details">
      <section class="phones-section" aria-labelledby="phones-title" data-feedback>
        <div class="section-heading"><h2 id="phones-title">已配对手机</h2><span id="phone-count" class="phone-count" hidden></span></div>
        <div id="relay-no-phones" class="phones-empty">
          <span class="phone-symbol" aria-hidden="true"></span>
          <p><strong>还没有配对的手机</strong><span>扫码连接后，可在这里撤销配对。</span></p>
        </div>
        <div id="relay-phones"></div>
      </section>
      <section class="service-status" aria-label="连接状态">
        <h2>连接状态</h2>
        <div class="status-cell" data-feedback>
          <div class="status-row"><div class="status-copy"><h3><span id="service-dot" class="status-dot" aria-hidden="true"></span>连接服务</h3><p id="service-state" role="status">正在读取…</p></div><button id="toggle-service" class="secondary-button" hidden disabled>启动服务</button></div>
          <p id="service-error" class="error-text" role="alert" hidden></p>
        </div>
        <div class="status-cell" data-feedback>
          <div class="status-copy"><h3><span id="sync-dot" class="status-dot" aria-hidden="true"></span>Codex 同步</h3><p id="sync-state" role="status">正在读取…</p></div>
          <p id="sync-error" class="error-text" role="alert" hidden></p>
          <div id="sync-actions" class="inline-actions" hidden><button id="retry-sync" class="secondary-button" disabled>重新连接</button></div>
        </div>
        <div class="status-cell">
          <div class="status-copy"><h3><span id="relay-dot" class="status-dot" aria-hidden="true"></span>云连接</h3><p id="relay-state" role="status">正在读取…</p></div>
        </div>
      </section>
    </div>
  </main>
  <dialog id="preferences-dialog" aria-labelledby="settings-title">
    <div class="preferences">
      <header class="preferences-header"><h2 id="settings-title">设置</h2><button id="close-settings" class="quiet-button" type="button" autofocus>关闭</button></header>
      <div class="preferences-body">
        <section id="relay-settings" class="preferences-section" aria-labelledby="connection-settings-title">
          <h3 id="connection-settings-title">手机连接</h3>
          <form id="relay-form" class="relay-form" data-feedback>
            <div class="relay-fields">
              <div><label for="relay-name">电脑名称</label><input id="relay-name" type="text" maxlength="48" placeholder="Codex 电脑" autocomplete="off"></div>
              <div><label for="relay-url">手机网页地址</label><input id="relay-url" type="url" placeholder="https://mobile.example.com" autocomplete="off" spellcheck="false" aria-describedby="relay-url-hint"></div>
            </div>
            <p id="relay-url-hint" class="hint">填写自己部署的 HTTPS 地址。留空并保存可关闭云连接。</p>
            <details id="legacy-relay-token" hidden><summary>旧版接入密钥</summary><div class="details-body"><label for="relay-token">替换密钥</label><input id="relay-token" type="password" autocomplete="off" placeholder="留空保留已保存的密钥"></div></details>
            <button id="save-relay" class="secondary-button" type="submit" disabled>保存连接</button>
          </form>
        </section>
        <section class="preferences-section" aria-labelledby="general-title">
          <h3 id="general-title">通用</h3>
          <div class="preference-group" data-feedback>
            <label class="startup-row" for="launch-on-login">
              <span><strong id="startup-title">登录时启动</strong><span id="startup-hint" class="hint" hidden></span></span>
              <span class="startup-switch">
                <input id="launch-on-login" type="checkbox" role="switch" aria-labelledby="startup-title" aria-describedby="startup-hint" disabled>
                <span class="startup-track" aria-hidden="true"></span>
              </span>
            </label>
          </div>
          <div class="desktop-row"><label for="theme">外观</label><select id="theme"><option value="system">跟随系统</option><option value="light">浅色</option><option value="dark">深色</option></select></div>
        </section>
        <section class="preferences-section" aria-labelledby="updates-title">
          <div class="section-heading"><h3 id="updates-title">软件更新</h3><span id="version" class="version"></span></div>
          <div class="update-main" data-feedback>
            <p id="update-status" class="hint" role="status">正在读取版本信息…</p>
            <div class="inline-actions"><button id="check-update" class="secondary-button" disabled>检查更新</button><button id="apply-update" class="primary-button" hidden>安装并重启</button></div>
            <details id="release-preview" hidden>
              <summary><span id="release-title">更新内容</span></summary>
              <div class="details-body"><p id="release-date" class="hint"></p><div id="release-notes" class="release-notes"></div><button id="release-page" class="quiet-button" type="button">在 GitHub 查看版本</button></div>
            </details>
          </div>
        </section>
        <section id="desktop-launch-settings" class="preferences-section" data-feedback hidden>
          <label class="startup-row" for="desktop-launch">
            <span><strong id="desktop-launch-title">Codex 自动接入</strong><span id="desktop-launch-hint" class="hint"></span></span>
            <span class="startup-switch"><input id="desktop-launch" type="checkbox" role="switch" aria-labelledby="desktop-launch-title" aria-describedby="desktop-launch-hint" disabled><span class="startup-track" aria-hidden="true"></span></span>
          </label>
        </section>
        <details id="service-settings" class="preferences-section"><summary>连接服务<span class="summary-hint">端口、访问范围与启动</span></summary>
            <form id="service-form" class="details-body" data-feedback>
              <div class="field-row"><div><label for="port">端口</label><input id="port" type="number" min="1" max="65535" required></div><div><label for="bind">访问范围</label><select id="bind" aria-describedby="bind-hint"><option value="0.0.0.0">本机与局域网</option><option value="127.0.0.1">仅本机</option><option value="::">IPv4 与 IPv6</option></select></div></div>
              <p id="bind-hint" class="hint">仅使用云连接时可选「仅本机」；手机直连需允许局域网访问。</p>
              <label class="checkbox-label"><input id="auto-start" type="checkbox">打开程序时自动启动连接服务</label>
              <p class="hint">更改端口或访问范围会重启连接服务，手机会短暂断开。</p>
              <div class="inline-actions"><button id="save-service" class="secondary-button" type="submit" disabled>保存设置</button><button id="settings-service" class="quiet-button" type="button" disabled>停止服务</button></div>
            </form>
        </details>
      </div>
    </div>
  </dialog>`;

const element = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const button = (id: string) => element<HTMLButtonElement>(id);
const field = (id: string) => element<HTMLInputElement>(id);
const preferences = element<HTMLDialogElement>('preferences-dialog');
const pairedDate = new Intl.DateTimeFormat('zh-CN', { month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit' });
let state: Status | undefined;
let relayPairing: RelayPairing | undefined;
let inviteVisible = false;
let invitedPhoneCount = 0;
let phoneRows = '';
let confirmPhone = '';
let busy = false;
let startupEnabled: boolean | undefined;
const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');

function feedback(text: string, error = false) {
  const changed = element('notice').textContent !== text;
  element('notice').textContent = text;
  element('notice').hidden = !text;
  element('notice').classList.toggle('error-text', error);
  if (text && changed) {
    element('notice').getAnimations().forEach(animation => animation.cancel());
    element('notice').animate([{ opacity: .35, transform: reducedMotion.matches ? 'none' : 'translateY(4px)' }, { opacity: 1, transform: 'none' }], { duration: reducedMotion.matches ? 100 : 200, easing: 'cubic-bezier(.16,1,.3,1)' });
  }
  if (error) element('notice').scrollIntoView({ block: 'nearest', behavior: 'instant' });
}
function errorMessage(error: unknown) {
  if (typeof error === 'object' && error && 'message' in error) return String(error.message);
  return typeof error === 'string' ? error : '操作未完成，请重试。';
}
function statusText(id: string, text: string) {
  const output = element(id);
  if (output.textContent !== text) output.textContent = text;
}
function render() {
  if (!state) return;
  const running = state.service.running;
  const synced = state.sync.state === 'connected';
  const launching = state.sync.state === 'connecting';
  const syncError = state.sync.state === 'error' ? state.sync.message : '';
  const syncLabel = launching ? '正在连接…' : synced ? '已同步' : '待同步';
  statusText('service-state', running ? '正在运行' : state.service.error ? '启动失败' : '已停止');
  element('service-dot').classList.toggle('connected', running);
  button('toggle-service').hidden = running;
  button('settings-service').textContent = running ? '停止服务' : '启动服务';
  statusText('sync-state', state.sync.state === 'history-only' ? state.sync.message : syncError ? '未连接' : syncLabel);
  element('sync-dot').classList.toggle('connected', synced);
  element('sync-error').hidden = !syncError;
  statusText('sync-error', syncError);
  element('service-error').hidden = !state.service.error;
  element('service-error').textContent = state.service.error || '';
  element('version').textContent = `v${state.version}`;
  const settings = state.updateSettings;
  const mode = settings.mode === 'source' ? '当前为源码运行。发布安装包后可使用自动升级。' : settings.mode === 'paused' ? '自动升级已暂停，可手动检查更新。' : '自动检查正式版本。';
  statusText('update-status', state.update.message || mode);
  if (state.update.phase === 'checking' && button('check-update').getAttribute('aria-busy') === 'true') feedback(state.update.message);
  element('update-status').classList.toggle('error-text', state.update.phase === 'error');
  element('release-preview').hidden = !state.release;
  if (state.release) {
    element('release-title').textContent = 'v' + state.release.version + ' 更新内容';
    element('release-notes').textContent = state.release.notes || '该版本未提供更新说明。';
    element('release-date').textContent = state.release.publishedAt ? '发布于 ' + new Date(state.release.publishedAt).toLocaleDateString() : '';
    element('release-date').hidden = !state.release.publishedAt;
  }
  button('apply-update').textContent = state.update.version ? '安装 v' + state.update.version + ' 并重启' : '安装并重启';
  for (const id of ['toggle-service', 'settings-service', 'save-service']) button(id).disabled = busy || launching;
  element('sync-actions').hidden = !running || synced || launching;
  button('retry-sync').disabled = busy || launching;
  button('check-update').disabled = busy || settings.mode === 'source' || state.update.phase === 'checking' || state.update.phase === 'applying';
  button('apply-update').hidden = state.update.phase !== 'ready';
  button('apply-update').disabled = busy;
  field('launch-on-login').disabled = busy || startupEnabled === undefined;
  if (!busy) field('launch-on-login').checked = startupEnabled === true;
  element('desktop-launch-settings').hidden = !state.desktopLaunch?.supported;
  field('desktop-launch').disabled = busy || launching;
  if (!busy) field('desktop-launch').checked = state.desktopLaunch?.enabled === true;
  statusText('desktop-launch-hint', state.desktopLaunch?.message || '');
  const relay = state.relay;
  statusText('relay-state', ({
    disabled: '未启用', stopped: '等待启动服务', unlinked: '待手机扫码', connecting: '连接中…', online: '已连接',
    offline: '暂未连接，正在重连',
  })[relay.state]);
  element('relay-dot').classList.toggle('connected', relay.state === 'online');
  element('device-name').hidden = !relay.name;
  element('device-label').textContent = relay.name;
  button('save-relay').disabled = busy;
  for (const id of ['relay-url', 'relay-name', 'relay-token']) field(id).disabled = button('save-relay').getAttribute('aria-busy') === 'true';
  for (const id of ['port', 'bind', 'auto-start']) field(id).disabled = button('save-service').getAttribute('aria-busy') === 'true';
  const invitation = !!relayPairing && relay.linkExpiresAt === relayPairing.expiresAt && relayPairing.expiresAt > Date.now();
  const restorePairingFocus = inviteVisible && !invitation && document.activeElement === button('copy-relay');
  button('link-phone').textContent = !relay.url ? '配置云连接' : invitation ? '刷新二维码' : '手机扫码连接';
  button('link-phone').disabled = busy;
  element('pair-art').hidden = invitation;
  element('invite-hint').hidden = !invitation;
  element('connect-hint').hidden = !!relay.url && running;
  element('connect-hint').textContent = relay.url ? '生成二维码时会启动连接服务。' : '先在设置中填写手机网页地址，再扫码连接。';
  button('copy-relay').disabled = busy;
  button('copy-relay').hidden = !invitation;
  element('relay-invite').hidden = !invitation;
  if (restorePairingFocus) button('link-phone').focus();
  if (inviteVisible && !invitation && relayPairing && !busy) {
    element('relay-access').append(element('notice'));
    feedback(relay.linkExpiresAt === relayPairing.expiresAt || relayPairing.expiresAt <= Date.now()
      ? '二维码已过期，可重新生成。'
      : relay.phones.length > invitedPhoneCount ? '手机配对成功。' : '二维码已失效，可重新生成。');
  }
  inviteVisible = invitation;
  element('legacy-relay-token').hidden = !relay.hasToken;
  element('relay-no-phones').hidden = relay.phones.length > 0;
  element('phone-count').hidden = !relay.phones.length;
  element('phone-count').textContent = `${relay.phones.length} 台`;
  const key = JSON.stringify([relay.phones, confirmPhone, busy]);
  if (phoneRows !== key) {
    phoneRows = key;
    const list = element('relay-phones');
    list.replaceChildren(...relay.phones.map(phone => {
      const row = document.createElement('div');
      row.className = 'phone-row';
      const symbol = document.createElement('span');
      symbol.className = 'phone-symbol';
      symbol.setAttribute('aria-hidden', 'true');
      const copy = document.createElement('p');
      const label = document.createElement('strong');
      label.textContent = phone.name;
      const date = document.createElement('span');
      date.textContent = pairedDate.format(phone.pairedAt) + ' 配对';
      copy.append(label, date);
      const revoke = document.createElement('button');
      revoke.type = 'button';
      revoke.className = 'quiet-button';
      revoke.textContent = confirmPhone === phone.id ? '确认撤销' : '撤销';
      revoke.disabled = busy;
      revoke.setAttribute('aria-label', revoke.textContent + ' ' + label.textContent);
      revoke.onclick = () => {
        const index = relay.phones.indexOf(phone);
        if (confirmPhone !== phone.id) {
          confirmPhone = phone.id;
          render();
          element('relay-phones').children[index]?.querySelector('button')?.focus();
          return;
        }
        void run('relay-phones', '正在撤销配对…', async () => {
          state = await request<Status>('revoke_relay_phone', { phone: phone.id });
          confirmPhone = '';
          feedback('配对已撤销。');
        }).then(() => {
          if (document.activeElement !== document.body) return;
          const rows = element('relay-phones').children;
          (rows[Math.min(index, rows.length - 1)]?.querySelector('button') || button('link-phone')).focus();
        });
      };
      const actions = document.createElement('div');
      actions.className = 'inline-actions';
      actions.append(revoke);
      if (confirmPhone === phone.id) {
        const cancel = document.createElement('button');
        cancel.className = 'quiet-button';
        cancel.textContent = '取消';
        cancel.onclick = () => {
          confirmPhone = '';
          render();
          element('relay-phones').children[relay.phones.indexOf(phone)]?.querySelector('button')?.focus();
        };
        actions.append(cancel);
      }
      row.append(symbol, copy, actions);
      return row;
    }));
  }
}
async function refresh(initial = false) {
  state = await request<Status>('status');
  if (initial) {
    field('port').value = String(state.config.port);element<HTMLSelectElement>('bind').value = state.config.bind;field('auto-start').checked = state.config.autoStart;
    field('relay-url').value = state.relay.url;
    field('relay-name').value = state.relay.name;
  }
  render();
}
async function run(id: string, message: string, action: () => Promise<void>) {
  if (busy) return;
  const trigger = element(id);
  const hadFocus = document.activeElement === trigger;
  busy = true;
  trigger.setAttribute('aria-busy', 'true');
  trigger.closest('[data-feedback]')?.append(element('notice'));
  feedback(message);render();
  try { await action(); } catch (error) { feedback(errorMessage(error), true); }
  finally {
    busy = false;
    trigger.removeAttribute('aria-busy');
    render();
    if (hadFocus && document.activeElement === document.body && trigger.getClientRects().length) trigger.focus();
  }
}
const on = (id: string, message: string, action: () => Promise<void>) => button(id).addEventListener('click', () => { void run(id, message, action); });
button('open-settings').addEventListener('click', () => preferences.showModal());
button('close-settings').addEventListener('click', () => preferences.close());
preferences.addEventListener('close', () => {
  if (preferences.contains(element('notice'))) {
    element('relay-access').append(element('notice'));
    if (!busy) feedback('');
  }
  button('open-settings').focus();
});
element('relay-form').addEventListener('submit', event => {
  event.preventDefault();
  void run('save-relay', '正在保存云连接…', async () => {
    state = await request<Status>('configure_relay', {
      url: field('relay-url').value, name: field('relay-name').value, token: field('relay-token').value,
    });
    field('relay-url').value = state.relay.url;
    field('relay-name').value = state.relay.name;
    field('relay-token').value = '';
    relayPairing = undefined;
    feedback(state.relay.url ? '云连接已保存。' : '云连接已关闭。');
  });
});
on('link-phone', '正在打开二维码…', async () => {
  if (!state?.relay.url) {
    preferences.showModal();
    field('relay-url').focus();
    feedback('');
    return;
  }
  const invite = await request<RelayPairing>('link_relay_desktop');
  await QRCode.toCanvas(element<HTMLCanvasElement>('relay-qr'), invite.link, { width: 240, margin: 4, errorCorrectionLevel: 'M' });
  relayPairing = invite;
  invitedPhoneCount = state?.relay.phones.length || 0;
  statusText('mobile-address', new URL(invite.link).host);
  await refresh();
  field('relay-url').value = state!.relay.url;
  field('relay-name').value = state!.relay.name;
  feedback('');
});
on('copy-relay', '', async () => {
  if (!relayPairing || state?.relay.linkExpiresAt !== relayPairing.expiresAt || relayPairing.expiresAt <= Date.now()) throw new Error('请重新生成二维码。');
  await navigator.clipboard.writeText(relayPairing.link);
  feedback('连接链接已复制。');
});
async function toggleService() {
  state = await request<Status>(state?.service.running ? 'stop_service' : 'start_service');
  feedback(state.service.running ? '连接服务已启动。' : '连接服务已停止。');
}
on('toggle-service', '正在启动连接服务…', toggleService);
on('settings-service', '正在切换连接服务…', toggleService);
on('retry-sync','正在重新连接 Codex…', async () => { state = await request<Status>('retry_sync');feedback(''); });
const themeSelect = element<HTMLSelectElement>('theme');
themeSelect.value = savedTheme || 'system';
function applyTheme() {
  document.documentElement.dataset.theme = themeSelect.value === 'system' ? (systemTheme.matches ? 'dark' : 'light') : themeSelect.value;
}
themeSelect.addEventListener('change', () => {
  if (themeSelect.value === 'system') localStorage.removeItem('codex-mobile-desktop-theme');
  else localStorage.setItem('codex-mobile-desktop-theme', themeSelect.value);
  applyTheme();
});
systemTheme.addEventListener('change', applyTheme);
field('launch-on-login').addEventListener('change', () => {
  const enabled = field('launch-on-login').checked;
  void run('launch-on-login','正在保存开机启动设置…',async () => {
    startupEnabled = await request<boolean>('set_autostart',{enabled});
    feedback(startupEnabled ? '已开启开机启动' : '已关闭开机启动');
  });
});
field('desktop-launch').addEventListener('change', () => {
  const enabled = field('desktop-launch').checked;
  void run('desktop-launch', '正在保存 Codex 启动配置…', async () => {
    state = await request<Status>('set_desktop_launch', { enabled });
    feedback(enabled ? '已配置。首次启用后请正常重开一次 Codex，以后无需固定启动顺序。' : '已撤销后续启动配置，当前连接和任务继续运行。');
  });
});

element('service-form').addEventListener('submit', event => { event.preventDefault();void run('save-service','正在保存服务设置…',async () => { const config:Config={...state!.config,port:Number(field('port').value),bind:element<HTMLSelectElement>('bind').value,autoStart:field('auto-start').checked};state=await request<Status>('save_config',{config});feedback('服务设置已保存。'); }); });
on('check-update','正在检查并准备更新…', async () => { state=await request<Status>('check_update');feedback(state.update.message); });
on('apply-update','正在完成当前请求并切换版本…', async () => { await request('apply_update'); });
on('release-page','', async () => { await request('open_external',{destination:'releases'}); });

try { await refresh(true); }
catch (error) { feedback(errorMessage(error), true); statusText('service-state', '无法读取桌面状态'); }
try { startupEnabled = await request<boolean>('get_autostart');render(); }
catch (error) { element('startup-hint').textContent=errorMessage(error);element('startup-hint').hidden=false;element('startup-hint').classList.add('error-text'); }
setInterval(() => { if ((!busy || button('check-update').getAttribute('aria-busy') === 'true') && !document.hidden) void refresh(!state).catch(error => feedback(errorMessage(error), true)); }, 2500);
