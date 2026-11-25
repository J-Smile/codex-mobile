import Noise from 'noise-handshake';
import { keyBytes, type DesktopApprovalInvite, type PhoneIdentity } from './devices';

const encoder = new TextEncoder();
const prologue = encoder.encode('codex-mobile-desktop-approval/v1');

export async function requestDesktopApproval(
  invitation: DesktopApprovalInvite,
  phone: PhoneIdentity,
  name: string,
  signal: AbortSignal,
  compareCode: (code: string) => void,
) {
  // 授权码和手机身份由电脑解密，云端只用码的摘要转发请求。
  const noise = new Noise('IK', true, phone);
  noise.initialise(prologue, keyBytes(invitation.publicKey));
  const proof = noise.send(encoder.encode(JSON.stringify({ ticket: invitation.ticket, name })));
  const [fingerprint, inviteHash] = await Promise.all([
    crypto.subtle.digest('SHA-256', proof),
    crypto.subtle.digest('SHA-256', encoder.encode(invitation.ticket)),
  ]);
  const code = Array.from(new Uint8Array(inviteHash), byte => byte.toString(16).padStart(2, '0')).join('');
  compareCode(String(new DataView(fingerprint).getUint32(0) % 1_000_000).padStart(6, '0'));
  const response = await fetch('/relay/account/desktop-approval', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({
      code,
      accountId: invitation.accountId,
      proof: btoa(String.fromCharCode(...proof)).replaceAll('+', '-').replaceAll('/', '_').replace(/=+$/, ''),
    }),
    cache: 'no-store',
    redirect: 'error',
    signal,
  });
  if (response.status === 401) throw new Error('电脑未同意这次授权。可以在电脑上重新生成二维码。');
  if (response.status === 403) throw new Error('这台电脑属于其他账号，请退出当前账号后重试。');
  if (response.status === 409 || response.status === 410) throw new Error('授权码已使用或过期，请在电脑上重新生成。');
  if (response.status === 404) throw new Error('请确认电脑在线，并使用新版桌面端生成授权码。');
  if (!response.ok) throw new Error('暂时无法完成电脑授权，请重新扫码。');
}
