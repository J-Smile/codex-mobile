const hexKey = /^[0-9a-f]{64}$/;
const inviteToken = /^[A-Za-z0-9_-]{43}$/;
interface InvitationDetails {
  publicKey: string;
  name: string;
  ticket: string;
  expiresAt: number;
}
export type DesktopApprovalInvite = InvitationDetails & { version: 2; accountId: string };
export type DesktopLinkInvite = InvitationDetails & { version: 3; claim: string | null };
export type Invitation = (InvitationDetails & { version: 1 }) | DesktopApprovalInvite | DesktopLinkInvite;
export interface PhoneIdentity {
  publicKey: Uint8Array<ArrayBuffer>;
  secretKey: Uint8Array<ArrayBuffer>;
}
export interface Desktop {
  id: string;
  publicKey: string;
  name: string;
  phone: PhoneIdentity;
}
export type PairingTarget = Pick<Desktop, 'id' | 'name'>;
export type PairingInvitation = Invitation & { id: string };

export function parseInvitation(value: string, origin = location.origin): Invitation {
  try {
    const link = new URL(value.trim());
    const raw = new URLSearchParams(link.hash.slice(1)).get('pair');
    if (link.origin !== origin || link.pathname !== '/' || link.search || link.username || link.password
      || !raw || !/^[A-Za-z0-9_-]{50,1024}$/.test(raw)) throw new Error();
    const bytes = Uint8Array.from(atob(raw.replaceAll('-', '+').replaceAll('_', '/')), code => code.charCodeAt(0));
    const invitation = JSON.parse(new TextDecoder().decode(bytes));
    if ((invitation.version !== 1 && invitation.version !== 2 && invitation.version !== 3)
      || (invitation.version === 2 && (typeof invitation.accountId !== 'string' || !/^[0-9a-f]{32}$/.test(invitation.accountId)))
      || (invitation.version === 3 && invitation.claim !== null
        && (typeof invitation.claim !== 'string' || !inviteToken.test(invitation.claim)))
      || typeof invitation.publicKey !== 'string' || !hexKey.test(invitation.publicKey)
      || typeof invitation.name !== 'string' || !invitation.name.trim() || invitation.name.length > 96
      || typeof invitation.ticket !== 'string' || !inviteToken.test(invitation.ticket)
      || !Number.isSafeInteger(invitation.expiresAt)) throw new Error();
    return invitation;
  } catch {
    throw new Error('请扫描此云端地址对应的 Codex Mobile 桌面二维码。');
  }
}

export function keyBytes(key: string) {
  return Uint8Array.from(key.match(/../g) || [], value => Number.parseInt(value, 16));
}

export async function desktopId(key: string) {
  const hash = await crypto.subtle.digest('SHA-256', keyBytes(key));
  return Array.from(new Uint8Array(hash), value => value.toString(16).padStart(2, '0')).join('');
}

export async function readPairingInvitation(value: string, target?: PairingTarget, origin = location.origin): Promise<PairingInvitation> {
  const invitation = parseInvitation(value, origin);
  if (invitation.expiresAt <= Date.now()) throw new Error('二维码已过期，请在电脑上重新生成。');
  const id = await desktopId(invitation.publicKey);
  if (target && target.id !== id) throw new Error(`请扫描「${target.name}」的云连接二维码。`);
  return { ...invitation, id };
}

function openDevices(accountId: string) {
  return new Promise<IDBDatabase>((resolve, reject) => {
    const request = indexedDB.open('codex-mobile-relay:' + accountId, 1);
    request.onupgradeneeded = () => request.result.createObjectStore('desktops', { keyPath: 'id' });
    request.onsuccess = () => resolve(request.result);
    request.onerror = () => reject(request.error);
  });
}

async function editDevices<T>(accountId: string, mode: IDBTransactionMode, action: (store: IDBObjectStore) => IDBRequest<T>) {
  const db = await openDevices(accountId);
  try {
    return await new Promise<T>((resolve, reject) => {
      const transaction = db.transaction('desktops', mode);
      const request = action(transaction.objectStore('desktops'));
      transaction.oncomplete = () => resolve(request.result);
      transaction.onabort = () => reject(transaction.error);
    });
  } finally { db.close(); }
}

export const loadDesktops = (accountId: string) => editDevices<Desktop[]>(accountId, 'readonly', store => store.getAll());
export const saveDesktop = (accountId: string, desktop: Desktop) => editDevices(accountId, 'readwrite', store => store.put(desktop));
export const forgetDesktop = (accountId: string, id: string) => editDevices(accountId, 'readwrite', store => store.delete(id));

export async function linkDesktop(desktopId: string, code: string) {
  const response = await fetch('/relay/account/desktop-link', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ desktopId, code }),
    cache: 'no-store',
    redirect: 'error',
  });
  if (response.status === 401) throw new Error('登录已过期，请重新登录后扫码。');
  if (response.status === 409 || response.status === 410) throw new Error('二维码已用过或过期，请在电脑上重新生成。');
  if (!response.ok) throw new Error('暂时无法添加这台电脑，请重试。');
}
