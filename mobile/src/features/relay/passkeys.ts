import {
  browserSupportsWebAuthn,
  startAuthentication,
  startRegistration,
  type PublicKeyCredentialCreationOptionsJSON,
  type PublicKeyCredentialRequestOptionsJSON,
} from '@simplewebauthn/browser';

export const supportsPasskeys = () => window.isSecureContext && browserSupportsWebAuthn();

const base = '/relay/account/passkeys';

async function request(path: string, body?: object) {
  const response = await fetch(base + path, {
    method: 'POST',
    cache: 'no-store',
    ...(body && { headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) }),
  });
  if (response.status === 429) throw new Error('请求过多，请稍后再试。');
  return response;
}

export async function signInWithPasskey() {
  const options = await request('/sign-in/options');
  if (!options.ok) throw new Error('无法开始通行密钥登录，请重试或使用网页登录密钥。');
  const { publicKey }: { publicKey: PublicKeyCredentialRequestOptionsJSON } = await options.json();
  const credential = await startAuthentication({ optionsJSON: publicKey }).catch(() => {
    throw new Error('未完成通行密钥验证。可重试，或使用网页登录密钥。');
  });
  const response = await request('/sign-in', credential);
  if (!response.ok) throw new Error('通行密钥验证失败，请重新登录。');
}

export async function createPasskey() {
  const options = await request('/create/options');
  if (options.status === 401 || options.status === 403) {
    throw new Error('请退出账号并重新登录，再创建通行密钥。');
  }
  if (!options.ok) throw new Error('无法开始创建通行密钥，请重试。');
  const { publicKey }: { publicKey: PublicKeyCredentialCreationOptionsJSON } = await options.json();
  const credential = await startRegistration({ optionsJSON: publicKey }).catch(() => {
    throw new Error('未创建通行密钥。若系统已保存此账号的通行密钥，可直接用它登录。');
  });
  const response = await request('/create', credential);
  if (!response.ok) throw new Error('通行密钥未能保存，请重新创建。');
}

export async function signUpWithPasskey() {
  const options = await request('/sign-up/options');
  if (!options.ok) throw new Error('无法开始创建账号，请刷新后重试。');
  const { publicKey }: { publicKey: PublicKeyCredentialCreationOptionsJSON } = await options.json();
  const credential = await startRegistration({ optionsJSON: publicKey }).catch(() => {
    throw new Error('通行密钥设置未完成。请用系统浏览器打开，并确认已开启通行密钥。');
  });
  const response = await request('/sign-up', credential);
  if (!response.ok) throw new Error('未能完成账号创建，请重试。');
}
