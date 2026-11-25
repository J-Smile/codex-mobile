import { relayDesktop } from '../../transport.ts';

export const conversationParams = (url: URL) => relayDesktop() ? new URLSearchParams(url.hash.slice(1)) : url.searchParams;

export function setConversationAddress(url: URL, threadId: string, projectId: string, newChat = false) {
  const params = conversationParams(url);
  for (const [key, value] of [['thread', threadId], ['project', projectId], ['new', newChat ? '1' : '']]) {
    if (value) params.set(key, value);
    else params.delete(key);
  }
  if (relayDesktop()) url.hash = params.toString();
}

export function conversationLink(threadId: string) {
  const desktop = relayDesktop();
  return (desktop ? '/?desktop=' + desktop + '#thread=' : '/?thread=') + encodeURIComponent(threadId);
}
