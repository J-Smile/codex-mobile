import { invoke } from '@tauri-apps/api/core';
export interface Config { port: number; bind: string; publicUrl: string; autoStart: boolean }
export interface UpdateSettings { version: string; mode: 'source' | 'paused' | 'automatic' }
export interface Status {
  version: string;
  service: { running: boolean; error: string | null };
  sync: { state: 'history-only' | 'stopped' | 'waiting' | 'connecting' | 'connected' | 'error'; message: string };
  runtime: { ready: boolean; message: string };
  desktopLaunch?: { supported: boolean; enabled: boolean; message: string };
  relay: {
    state: 'disabled' | 'stopped' | 'unlinked' | 'connecting' | 'online' | 'offline';
    url: string;
    name: string;
    hasToken: boolean;
    linkExpiresAt: number | null;
    phones: { id: string; name: string; pairedAt: number }[];
  };
  config: Config;
  update: { phase: 'idle' | 'checking' | 'current' | 'ready' | 'applying' | 'error'; version?: string; message: string };
  updateSettings: UpdateSettings;
  release?: { version: string; notes: string; publishedAt: string | null } | null;
}
export interface RelayPairing { link: string; expiresAt: number }
export const request = <T>(command: string, args?: Record<string, unknown>) => invoke<T>(command, args);
