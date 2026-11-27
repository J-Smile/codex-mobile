import QRCode from 'qrcode';
import { request, type Status, type RelayPairing } from './api';
import './style.css';

const savedTheme = localStorage.getItem('codex-mobile-desktop-theme');
const systemTheme = matchMedia('(prefers-color-scheme: dark)');
document.documentElement.dataset.theme = savedTheme || (systemTheme.matches ? 'dark' : 'light');
document.querySelector('#app')!.innerHTML = `
  <aside class="sidebar" aria-label="桌面导航">
    <div class="brand"><img src="/icon.svg" width="26" height="26" alt="">Codex Mobile</div>
    <div class="side-computer"><span class="monitor-symbol" aria-hidden="true"></span><div><strong id="device-label">我的电脑</strong></div></div>
    <nav aria-label="主导航">
      <button id="open-home" class="nav-button active" aria-current="page" type="button"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="m10 14 4-4m-6 6-1 1a4 4 0 0 1-6-6l4-4a4 4 0 0 1 6 0m2 10a4 4 0 0 0 6 0l4-4a4 4 0 0 0-6-6l-1 1"/></svg>手机连接</button>
      <button id="open-settings" class="nav-button" type="button"><svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.6" aria-hidden="true"><path d="M3 7h3m6 0h9M3 17h10m6 0h2"/><circle cx="9" cy="7" r="3"/><circle cx="16" cy="17" r="3"/></svg>设置</button>
    </nav>
    <p class="connection-summary" role="status"><span id="connection-dot" class="status-dot" aria-hidden="true"></span><span id="connection-summary">正在读取…</span></p>
  </aside>
  <main class="workspace">
    <div id="connection-page" class="content-page">
      <section id="relay-access" class="relay-access" aria-labelledby="connect-title" data-feedback>
        <div class="connect-copy">
          <h1 id="connect-title" tabindex="-1">添加手机</h1>
          <p id="connect-description" class="connect-description">正在读取连接配置…</p>
          <div class="connect-actions"><button id="link-phone" class="primary-button" type="button" disabled>添加手机</button><button id="copy-relay" class="quiet-button" type="button" hidden>复制连接链接</button></div>
          <p id="invite-hint" class="hint" hidden>手机网页 <strong id="mobile-address"></strong></p>
        </div>
        <div class="connection-stage">
          <div id="pair-art" class="pair-art" aria-hidden="true"><span class="phone-symbol"></span></div>
          <div id="relay-invite" class="relay-invite" hidden><canvas id="relay-qr" width="240" height="240" role="img" aria-label="电脑连接二维码"></canvas></div>
        </div>
        <p id="notice" class="notice" role="status" hidden></p>
      </section>
      <section class="phones-section" aria-labelledby="phones-title" data-feedback>
        <div class="section-heading"><h2 id="phones-title">已配对手机</h2><span id="phone-count" class="phone-count"></span></div>
        <div id="relay-no-phones" class="phones-empty"><span class="phone-symbol" aria-hidden="true"></span><p><strong>暂无已配对手机</strong></p></div>
        <div id="relay-phones"></div>
      </section>
      <section id="connection-issues" class="connection-issues" aria-label="连接提示" hidden>
        <div id="service-issue" class="status-cell" data-feedback hidden><div class="status-row"><div class="status-copy"><h3>连接服务</h3><p id="service-state" role="status"></p></div><button id="toggle-service" class="secondary-button" disabled>启动服务</button></div><p id="service-error" class="error-text" role="alert" hidden></p></div>
        <div id="sync-issue" class="status-cell" data-feedback hidden><div class="status-row"><div class="status-copy"><h3>Codex 连接</h3><p id="sync-state" role="status"></p></div><div id="sync-actions" hidden><button id="retry-sync" class="secondary-button" disabled>重新连接</button></div></div><p id="sync-error" class="error-text" role="alert" hidden></p></div>
        <div id="relay-issue" class="status-cell" hidden><div class="status-copy"><h3>手机网页</h3><p id="relay-state" role="status"></p></div></div>
      </section>
      <p id="background-hint" class="footer-note" hidden>关闭窗口后，连接服务继续运行。</p>
    </div>
    <section id="settings-page" class="content-page settings-page" aria-labelledby="settings-title" hidden>
      <h1 id="settings-title" tabindex="-1">设置</h1>
      <section class="preferences-section" aria-labelledby="connection-settings-title">
        <h2 id="connection-settings-title">连接</h2>
        <form id="relay-form" class="relay-form" data-feedback>
          <label for="relay-name">电脑名称<input id="relay-name" type="text" maxlength="48" placeholder="Codex 电脑" autocomplete="off"></label>
          <label for="relay-url">手机网页地址<input id="relay-url" type="url" placeholder="https://mobile.example.com" autocomplete="off" spellcheck="false" aria-describedby="relay-url-hint"></label>
          <p id="relay-url-hint" class="hint">填写手机上打开的 HTTPS 网页地址；清空并保存可关闭云连接。</p>
          <button id="save-relay" class="primary-button" type="submit" disabled>保存连接</button>
        </form>
      </section>
      <section class="preferences-section" aria-labelledby="general-title">
        <h2 id="general-title">偏好</h2>
        <div class="preference-group" data-feedback><label class="startup-row" for="launch-on-login"><span><strong id="startup-title">登录时启动</strong><span id="startup-hint" class="hint" hidden></span></span><span class="startup-switch"><input id="launch-on-login" type="checkbox" role="switch" aria-labelledby="startup-title" aria-describedby="startup-hint" disabled><span class="startup-track" aria-hidden="true"></span></span></label></div>
        <div class="desktop-row"><label for="theme">外观</label><select id="theme"><option value="system">跟随系统</option><option value="light">浅色</option><option value="dark">深色</option></select></div>
      </section>
      <section class="preferences-section" aria-labelledby="updates-title">
        <div class="section-heading"><h2 id="updates-title">软件更新</h2><span id="version" class="version"></span></div>
        <div class="update-main" data-feedback><p id="update-status" class="hint" role="status">正在读取版本信息…</p><div class="inline-actions"><button id="check-update" class="secondary-button" type="button" disabled>检查更新</button><button id="apply-update" class="primary-button" type="button" hidden>安装并重启</button></div><details id="release-preview" hidden><summary><span id="release-title">更新内容</span></summary><div class="details-body"><p id="release-date" class="hint"></p><div id="release-notes" class="release-notes"></div><button id="release-page" class="quiet-button" type="button">在 GitHub 查看版本</button></div></details></div>
      </section>
    </section>
  </main>
  <dialog id="restart-reminder" aria-labelledby="reminder-title" aria-describedby="reminder-copy"><span class="monitor-symbol" aria-hidden="true"></span><h2 id="reminder-title">重开一次 Codex</h2><p id="reminder-copy">请在当前任务结束后，正常重开一次 Codex。以后 Codex 和 Mobile 可按任意顺序打开。</p><form method="dialog"><button class="primary-button" autofocus>知道了</button></form></dialog>`;

const element = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const button = (id: string) => element<HTMLButtonElement>(id);
const field = (id: string) => element<HTMLInputElement>(id);
const reminder = element<HTMLDialogElement>('restart-reminder');
const reminderKey = 'codex-mobile-desktop-restart-notice';
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
  const restartRequired = state.sync.restartRequired === true;
  const syncError = !restartRequired && state.sync.state === 'error' ? state.sync.message : '';
  const syncLabel = restartRequired ? '等待 Codex 重开' : launching ? '正在连接…' : synced ? '已连接' : '等待 Codex 连接';
  statusText('service-state', running ? '正在运行' : state.service.error ? '启动失败' : '已停止');
  element('service-issue').hidden = running;
  element('sync-issue').hidden = !running || synced;
  element('background-hint').hidden = !running;
  statusText('sync-state', state.sync.state === 'history-only' ? state.sync.message : syncError ? '未连接' : syncLabel);
  button('retry-sync').textContent = restartRequired ? '查看提醒' : '重新连接';
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
  button('toggle-service').disabled = busy || launching;
  element('sync-actions').hidden = !running || synced || launching;
  button('retry-sync').disabled = busy || launching;
  button('check-update').disabled = busy || settings.mode === 'source' || state.update.phase === 'checking' || state.update.phase === 'applying';
  button('apply-update').hidden = state.update.phase !== 'ready';
  button('apply-update').disabled = busy;
  field('launch-on-login').disabled = busy || startupEnabled === undefined;
  if (!busy) field('launch-on-login').checked = startupEnabled === true;
  if (restartRequired && !busy && !reminder.open && !localStorage.getItem(reminderKey)) reminder.showModal();
  const relay = state.relay;
  statusText('relay-state', ({
    disabled: '未启用', stopped: '等待启动服务', unlinked: '待手机扫码', connecting: '连接中…', online: '已连接',
    offline: '暂未连接，正在重连',
  })[relay.state]);
  const relayIssue = !!relay.url && ['connecting', 'offline', 'stopped'].includes(relay.state);
  element('relay-issue').hidden = !relayIssue;
  element('connection-issues').hidden = running && synced && !relayIssue;
  const ready = running && synced && relay.state === 'online';
  element('connection-dot').classList.toggle('connected', ready);
  statusText('connection-summary', !running ? '连接服务未启动' : !synced ? syncLabel : !relay.url ? '待配置手机网页' : relay.state === 'unlinked' ? '待添加手机' : ready ? '连接就绪' : '手机连接恢复中');
  element('device-label').textContent = relay.name || '我的电脑';
  button('save-relay').disabled = busy;
  for (const id of ['relay-url', 'relay-name']) field(id).disabled = button('save-relay').getAttribute('aria-busy') === 'true';
  const invitation = !!relayPairing && relay.linkExpiresAt === relayPairing.expiresAt && relayPairing.expiresAt > Date.now();
  const restorePairingFocus = inviteVisible && !invitation && document.activeElement === button('copy-relay');
  button('link-phone').textContent = !relay.url ? '设置网页地址' : invitation ? '刷新二维码' : '生成二维码';
  button('link-phone').disabled = busy;
  element('pair-art').hidden = invitation;
  element('invite-hint').hidden = !invitation;
  statusText('connect-description', relay.url ? '用手机网页的「添加电脑」扫描二维码。' : '填写手机网页地址，开始连接。');
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
  element('relay-no-phones').hidden = relay.phones.length > 0;
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
function showPage(settings: boolean) {
  element('connection-page').hidden = settings;
  element('settings-page').hidden = !settings;
  for (const id of ['open-home', 'open-settings']) {
    const active = (id === 'open-settings') === settings;
    button(id).classList.toggle('active', active);
    if (active) button(id).setAttribute('aria-current', 'page');
    else button(id).removeAttribute('aria-current');
  }
  element(settings ? 'settings-title' : 'connect-title').focus();
}
button('open-settings').addEventListener('click', () => showPage(true));
button('open-home').addEventListener('click', () => showPage(false));
for (const event of ['submit', 'cancel']) {
  reminder.addEventListener(event, () => localStorage.setItem(reminderKey, 'acknowledged'));
}

element('relay-form').addEventListener('submit', event => {
  event.preventDefault();
  void run('save-relay', '正在保存云连接…', async () => {
    state = await request<Status>('configure_relay', {
      url: field('relay-url').value, name: field('relay-name').value,
    });
    field('relay-url').value = state.relay.url;
    field('relay-name').value = state.relay.name;
    relayPairing = undefined;
    feedback(state.relay.url ? '云连接已保存。' : '云连接已关闭。');
  });
});
on('link-phone', '正在打开二维码…', async () => {
  if (!state?.relay.url) {
    showPage(true);
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
on('toggle-service', '正在启动连接服务…', async () => {
  state = await request<Status>('start_service');
  feedback(state.service.running ? '' : '连接服务未启动，请重试。');
});
button('retry-sync').addEventListener('click', () => {
  if (state?.sync.restartRequired) reminder.showModal();
  else void run('retry-sync', '正在重新连接 Codex…', async () => { state = await request<Status>('retry_sync'); feedback(''); });
});
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
    feedback('');
  });
});
on('check-update','正在检查并准备更新…', async () => { state=await request<Status>('check_update');feedback(state.update.message); });
on('apply-update','正在完成当前请求并切换版本…', async () => { await request('apply_update'); });
on('release-page','', async () => { await request('open_external',{destination:'releases'}); });

try { await refresh(true); }
catch (error) { feedback(errorMessage(error), true); statusText('connection-summary', '无法读取桌面状态'); }
try { startupEnabled = await request<boolean>('get_autostart');render(); }
catch (error) { element('startup-hint').textContent=errorMessage(error);element('startup-hint').hidden=false;element('startup-hint').classList.add('error-text'); }
setInterval(() => { if ((!busy || button('check-update').getAttribute('aria-busy') === 'true') && !document.hidden) void refresh(!state).catch(error => feedback(errorMessage(error), true)); }, 2500);
