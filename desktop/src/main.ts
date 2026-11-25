import QRCode from 'qrcode';
import { request, type Status, type Pairing, type RelayPairing, type Config } from './api';
import './style.css';

const savedTheme = localStorage.getItem('codex-mobile-desktop-theme');
document.documentElement.dataset.theme = savedTheme || (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light');
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
          <div class="status-row sync-row"><div class="status-copy"><h3><span id="sync-dot" class="status-dot" aria-hidden="true"></span>Codex 同步</h3><p id="sync-state" role="status">正在读取…</p></div><button id="open-codex-action" class="secondary-button" disabled>打开 Codex</button></div>
          <p id="sync-error" class="error-text" role="alert" hidden></p>
          <div id="sync-actions" class="inline-actions" hidden><button id="retry-sync" class="secondary-button" disabled>重新连接</button></div>
        </div>
        <div class="status-cell">
          <div class="status-copy"><h3><span id="relay-dot" class="status-dot" aria-hidden="true"></span>云连接</h3><p id="relay-state" role="status">正在读取…</p></div>
        </div>
      </section>
    </div>
    <footer>关闭窗口后继续在托盘运行</footer>
  </main>
  <dialog id="preferences-dialog" aria-labelledby="settings-title">
    <div class="preferences">
      <header class="preferences-header"><h2 id="settings-title">设置</h2><button id="close-settings" class="quiet-button" type="button" autofocus>关闭</button></header>
      <div class="preferences-body">
        <section class="preferences-section" aria-labelledby="general-title">
          <h3 id="general-title">通用</h3>
          <div class="preference-group" data-feedback>
            <label class="startup-row" for="launch-on-login">
              <span><strong id="startup-title">开机启动</strong><span id="startup-hint" class="hint">登录后在托盘运行</span></span>
              <span class="startup-switch">
                <input id="launch-on-login" type="checkbox" role="switch" aria-labelledby="startup-title" aria-describedby="startup-hint" disabled>
                <span class="startup-track" aria-hidden="true"></span>
              </span>
            </label>
          </div>
          <div class="desktop-row"><span>外观</span><button id="theme" class="quiet-button" type="button" aria-label="切换主题">切换主题</button></div>
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
          <details id="update-settings"><summary>更新来源<span id="token-state" class="summary-info"></span></summary>
            <div class="details-body" data-feedback>
              <form id="token-form"><label for="token">私有仓库 GitHub token</label><input id="token" type="password" autocomplete="off" placeholder="输入新的 token" aria-describedby="token-hint"><p id="token-hint" class="hint">仅需 Contents: read 权限，只保存在本机</p><div class="inline-actions"><button id="save-token" class="secondary-button" type="submit" disabled>验证并保存</button><button id="token-help" class="quiet-button" type="button">生成 token</button><button id="remove-token" class="quiet-button" type="button" hidden>移除 token</button></div></form>
              <div id="remove-token-confirm" class="inline-confirm" hidden><p>移除后停止自动检查私有仓库的更新。</p><button id="confirm-remove-token" class="secondary-button">确认移除</button><button id="cancel-remove-token" class="quiet-button">取消</button></div>
            </div>
          </details>
        </section>
        <section class="preferences-section" aria-labelledby="connection-settings-title">
          <h3 id="connection-settings-title">连接与维护</h3>
          <details id="relay-settings"><summary>云连接设置</summary>
            <div class="details-body" data-feedback>
              <form id="relay-form" class="relay-form">
                <div><label for="relay-name">电脑名称</label><input id="relay-name" type="text" maxlength="48" placeholder="例如工作电脑" autocomplete="off"></div>
                <div><label for="relay-url">手机网页地址</label><input id="relay-url" type="url" placeholder="https://mobile.example.com" autocomplete="off"></div>
                <div id="legacy-relay-token" hidden><label for="relay-token">原有电脑接入密钥</label><input id="relay-token" type="password" autocomplete="off" placeholder="已保存，填写可替换"></div>
                <p class="hint">填写自己部署的手机网页 HTTPS 地址后启用云连接。清空地址可关闭。</p>
                <button id="save-relay" class="secondary-button" type="submit" disabled>保存云连接</button>
              </form>
            </div>
          </details>
          <details id="lan-settings"><summary>局域网连接</summary>
            <div id="lan-pairing" class="pairing-grid details-body" role="region" aria-label="局域网连接">
              <div class="scan-column">
                <div class="qr-frame"><canvas id="qr" width="216" height="216" aria-label="局域网连接二维码" role="img" hidden></canvas><p id="qr-empty">正在读取连接信息…</p></div>
              </div>
              <div class="key-field" data-feedback>
                <label for="key">连接密钥</label>
                <p id="key-hint" class="hint">手机与电脑在同一网络时可扫码，也可以复制密钥。</p>
                <div class="input-action"><input id="key" type="password" readonly autocomplete="off" aria-describedby="key-hint"><button id="reveal-key" class="quiet-button" type="button" aria-pressed="false">显示</button></div>
                <button id="copy-key" class="secondary-button" disabled><span id="copy-label">复制密钥</span></button>
              </div>
            </div>
          </details>
          <details id="service-settings"><summary>服务设置</summary>
            <form id="service-form" class="details-body" data-feedback>
              <div class="field-row"><div><label for="port">端口</label><input id="port" type="number" min="1" max="65535" required></div><div><label for="bind">访问范围</label><select id="bind"><option value="0.0.0.0">本机与内网</option><option value="127.0.0.1">仅本机</option><option value="::">IPv4 与 IPv6</option></select></div></div>
              <label class="checkbox-label"><input id="auto-start" type="checkbox">打开程序时自动启动连接服务</label>
              <div class="inline-actions"><button id="save-service" class="secondary-button" type="submit" disabled>保存设置</button><button id="settings-service" class="quiet-button" type="button" disabled>停止服务</button></div>
            </form>
          </details>
          <details><summary>诊断与密钥</summary><div class="details-body" data-feedback>
            <div class="desktop-row"><div><strong>官方 Codex</strong><p id="runtime-state" class="hint">正在读取状态…</p></div><button id="open-codex" class="quiet-button" disabled>打开 Codex</button></div>
            <div class="diagnostic-actions"><button id="copy-diagnostics" class="secondary-button" disabled>复制诊断信息</button><button id="rotate-key" class="quiet-button" disabled>刷新连接密钥</button></div>
            <div id="rotate-confirm" class="inline-confirm" hidden><p>旧二维码和密钥将失效，已连接的手机需要重新配对。</p><button id="confirm-rotate" class="secondary-button">确认刷新</button><button id="cancel-rotate" class="quiet-button">取消</button></div>
          </div></details>
        </section>
      </div>
    </div>
  </dialog>`;

const element = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
const button = (id: string) => element<HTMLButtonElement>(id);
const field = (id: string) => element<HTMLInputElement>(id);
const preferences = element<HTMLDialogElement>('preferences-dialog');
const pairedDate = new Intl.DateTimeFormat('zh-CN', { month: 'long', day: 'numeric', hour: '2-digit', minute: '2-digit' });
let state: Status | undefined;
let pairing: Pairing | undefined;
let relayPairing: RelayPairing | undefined;
let inviteVisible = false;
let invitedPhoneCount = 0;
let phoneRows = '';
let confirmPhone = '';
let busy = false;
let lastQr = '';
let startupEnabled: boolean | undefined;
const reducedMotion = matchMedia('(prefers-reduced-motion: reduce)');
let copyReset: ReturnType<typeof setTimeout>;

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
  statusText('runtime-state', launching ? '正在连接 Codex…' : syncError || state.runtime.message);
  element('version').textContent = `v${state.version}`;
  const settings = state.updateSettings;
  const mode = settings.mode === 'source' ? '当前为源码运行。发布安装包后可使用自动升级。' : settings.mode === 'paused' ? '自动升级已暂停。' : settings.configured ? '自动检查私有仓库，更新时保留现有聊天与配对信息。' : '配置 token 后可检查私有仓库的正式版本。';
  statusText('update-status', state.update.message || mode);
  element('update-status').classList.toggle('error-text', state.update.phase === 'error');
  element('token-state').textContent = settings.configured ? '已连接私有仓库' : '待配置';
  element('release-preview').hidden = !state.release;
  if (state.release) {
    element('release-title').textContent = 'v' + state.release.version + ' 更新内容';
    element('release-notes').textContent = state.release.notes || '该版本未提供更新说明。';
    element('release-date').textContent = state.release.publishedAt ? '发布于 ' + new Date(state.release.publishedAt).toLocaleDateString() : '';
    element('release-date').hidden = !state.release.publishedAt;
  }
  button('apply-update').textContent = state.update.version ? '安装 v' + state.update.version + ' 并重启' : '安装并重启';
  field('token').disabled = busy || settings.source === 'environment';
  element('token-hint').textContent = settings.source === 'environment' ? '当前 token 由电脑环境变量提供。请在电脑移除环境变量并重启后，再使用这里的配置。' : `${settings.configured ? 'token 已保存。' : ''}仅需 J-Smile/codex-mobile 的 Contents: read 权限，只保存在本机。`;
  for (const id of ['toggle-service', 'settings-service', 'save-service', 'copy-diagnostics', 'rotate-key', 'confirm-remove-token', 'confirm-rotate']) button(id).disabled = busy;
  for (const id of ['toggle-service', 'settings-service', 'save-service']) button(id).disabled = busy || launching;
  button('open-codex').disabled = busy || launching;
  button('open-codex-action').disabled = busy || launching;
  button('save-token').disabled = busy || settings.source === 'environment' || !field('token').value.trim();
  element('sync-actions').hidden = state.sync.state !== 'error';
  button('retry-sync').disabled = busy || launching;
  button('copy-key').disabled = busy || !pairing;
  button('check-update').disabled = busy || !settings.configured || settings.mode === 'source' || state.update.phase === 'checking';
  button('apply-update').hidden = state.update.phase !== 'ready';
  button('apply-update').disabled = busy;
  button('remove-token').hidden = !settings.configured || settings.source !== 'file';
  button('remove-token').disabled = busy;
  field('launch-on-login').disabled = busy || startupEnabled === undefined;
  if (!busy) field('launch-on-login').checked = startupEnabled === true;
  element('qr').hidden = !pairing || !running;
  element('qr-empty').hidden = !!pairing && running;
  element('qr-empty').textContent = !running ? '启动连接服务后\n显示二维码' : '正在读取连接密钥…';
  const relay = state.relay;
  statusText('relay-state', ({
    disabled: '未启用', stopped: '等待启动服务', unlinked: '待手机扫码', connecting: '连接中…', online: '已连接',
    offline: '暂未连接，正在重连',
  })[relay.state]);
  element('relay-dot').classList.toggle('connected', relay.state === 'online');
  element('device-name').hidden = !relay.name;
  element('device-label').textContent = relay.name;
  button('save-relay').disabled = busy;
  const invitation = !!relayPairing && relay.linkExpiresAt === relayPairing.expiresAt && relayPairing.expiresAt > Date.now();
  const restorePairingFocus = inviteVisible && !invitation && document.activeElement === button('copy-relay');
  button('link-phone').textContent = !relay.url ? '配置云连接' : invitation ? '刷新二维码' : '手机扫码连接';
  button('link-phone').disabled = busy;
  element('pair-art').hidden = invitation;
  element('invite-hint').hidden = !invitation;
  element('connect-hint').hidden = !!relay.url && running;
  element('connect-hint').textContent = relay.url ? '生成二维码时会启动连接服务。' : '先配置自己的中转服务器；同一网络可在设置中使用局域网连接。';
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
async function updateQr() {
  if (pairing && lastQr !== pairing.accessKey) {
    await QRCode.toCanvas(element<HTMLCanvasElement>('qr'), pairing.accessKey, { width: 216, margin: 2, errorCorrectionLevel: 'M' });
    lastQr = pairing.accessKey;
    element('qr').animate([{ opacity: .35, transform: reducedMotion.matches ? 'none' : 'scale(.97)' }, { opacity: 1, transform: 'none' }], { duration: reducedMotion.matches ? 100 : 260, easing: 'cubic-bezier(.16,1,.3,1)' });
  }
  render();
}
async function refreshPairing() {
  pairing = await request<Pairing>('pairing');field('key').value = pairing.accessKey;
  await updateQr();
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
  if (field('key').type === 'text') button('reveal-key').click();
  element('rotate-confirm').hidden = true;
  element('remove-token-confirm').hidden = true;
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
    field('relay-token').value = '';
    relayPairing = undefined;
    feedback(state.relay.url ? '云连接已保存。' : '云连接已关闭。');
  });
});
on('link-phone', '正在打开二维码…', async () => {
  if (!state?.relay.url) {
    element<HTMLDetailsElement>('relay-settings').open = true;
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
  await refreshPairing();
  feedback(state.service.running ? '连接服务已启动。' : '连接服务已停止。');
}
on('toggle-service', '正在启动连接服务…', toggleService);
on('settings-service', '正在切换连接服务…', toggleService);
on('retry-sync','正在重新连接 Codex…', async () => { state = await request<Status>('retry_sync');feedback(''); });
async function openCodex() {
  state = await request<Status>('open_codex');
  feedback('');
}
on('open-codex', '正在打开 Codex…', openCodex);
on('open-codex-action', '正在打开 Codex…', openCodex);
on('copy-key','', async () => {
  await navigator.clipboard.writeText(pairing!.accessKey);
  clearTimeout(copyReset);
  statusText('copy-label', '已复制');
  copyReset = setTimeout(() => statusText('copy-label', '复制密钥'), 1800);
  feedback('连接密钥已复制。');
});
on('copy-diagnostics','', async () => { const text = await request<string>('diagnostics');await navigator.clipboard.writeText(text);feedback('诊断信息已复制。'); });
button('reveal-key').addEventListener('click', () => { const show = field('key').type === 'password';field('key').type = show ? 'text' : 'password';button('reveal-key').textContent = show ? '隐藏' : '显示';button('reveal-key').setAttribute('aria-pressed',String(show)); });
button('theme').addEventListener('click', () => { const theme = document.documentElement.dataset.theme === 'dark' ? 'light' : 'dark';document.documentElement.dataset.theme = theme;localStorage.setItem('codex-mobile-desktop-theme',theme); });
field('token').addEventListener('input', render);
field('launch-on-login').addEventListener('change', () => {
  const enabled = field('launch-on-login').checked;
  void run('launch-on-login','正在保存开机启动设置…',async () => {
    startupEnabled = await request<boolean>('set_autostart',{enabled});
    feedback(startupEnabled ? '已开启开机启动' : '已关闭开机启动');
  });
});
element('service-form').addEventListener('submit', event => { event.preventDefault();void run('save-service','正在保存服务设置…',async () => { const config:Config={...state!.config,port:Number(field('port').value),bind:element<HTMLSelectElement>('bind').value,autoStart:field('auto-start').checked};state=await request<Status>('save_config',{config});await refreshPairing();feedback('服务设置已保存。'); }); });
button('rotate-key').addEventListener('click', () => { element('rotate-confirm').hidden=false;button('confirm-rotate').focus(); });
button('cancel-rotate').addEventListener('click', () => { element('rotate-confirm').hidden=true;button('rotate-key').focus(); });
on('confirm-rotate','正在刷新连接密钥…', async () => { pairing=await request<Pairing>('rotate_key');field('key').value=pairing.accessKey;await updateQr();element('rotate-confirm').hidden=true;feedback('连接密钥已刷新，请重新连接手机。'); });
on('check-update','正在检查并准备更新…', async () => { state=await request<Status>('check_update');feedback(state.update.message); });
on('apply-update','正在完成当前请求并切换版本…', async () => { await request('apply_update'); });
on('release-page','', async () => { await request('open_external',{destination:'releases'}); });
on('token-help','', async () => { await request('open_external',{destination:'token'});feedback('在 GitHub 选择 codex-mobile 仓库，并授予 Contents 的只读权限。'); });
element('token-form').addEventListener('submit', event => { event.preventDefault();void run('save-token','正在验证并保存 token…',async () => { await request('save_update_token',{token:field('token').value});field('token').value='';await refresh();feedback('token 已验证并保存。'); }); });
button('remove-token').addEventListener('click', () => { element('remove-token-confirm').hidden=false;button('confirm-remove-token').focus(); });
button('cancel-remove-token').addEventListener('click', () => { element('remove-token-confirm').hidden=true;button('remove-token').focus(); });
on('confirm-remove-token','正在移除 token…', async () => { await request('remove_update_token');await refresh();element('remove-token-confirm').hidden=true;feedback('token 已移除。'); });

try { await refresh(true);await refreshPairing(); }
catch (error) { feedback(errorMessage(error), true); statusText('service-state', '无法读取桌面状态'); statusText('qr-empty', '请从桌面程序打开此页面'); }
try { startupEnabled = await request<boolean>('get_autostart');render(); }
catch (error) { element('startup-hint').textContent=errorMessage(error);element('startup-hint').classList.add('error-text'); }
setInterval(() => { if (!busy && !document.hidden) void refresh(!state).catch(error => feedback(errorMessage(error), true)); }, 2500);
