import formats from '../../../../../preview-formats.json' with { type: 'json' };

export type PreviewKind = keyof typeof formats;
const extensions = new Map(Object.entries(formats).flatMap(([kind, values]) =>
  values.map(extension => [extension, kind as PreviewKind] as const)));

export function previewFile(reference: string) {
  if (!reference || reference.startsWith('data:')) return;
  let path: string;
  try { path = decodeURIComponent(reference.split(/[?#]/, 1)[0]!.replace(/:\d+(?::\d+)?$/, '')); }
  catch { return; }
  const name = path.split(/[\\/]/).at(-1) || '';
  const kind = extensions.get(name.split('.').at(-1)!.toLowerCase());
  const remote = /^[\\/]{2}/.test(reference) || (/^[\w+.-]+:/.test(reference) && !/^(?:[a-z]:[\\/]|file:\/\/\/)/i.test(reference));
  if (kind && (kind === 'disabled' || !remote)) return { name, kind };
}
