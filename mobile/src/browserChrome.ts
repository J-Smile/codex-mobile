import { computed, onScopeDispose, ref, watch } from 'vue';

type Theme = 'light' | 'dark';
type Surface = 'navigation' | 'chat' | 'pairing';

export function syncBrowserChrome(theme: Theme, surface: Surface) {
  const root = document.documentElement;
  root.dataset.theme = theme;
  root.dataset.surface = surface;
  const color = getComputedStyle(root).getPropertyValue('--chrome-background').trim();
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', color);
  document.querySelector('meta[name="color-scheme"]')?.setAttribute('content', theme);
}

export function useBrowserTheme(surface: () => Surface) {
  const key = 'codex-mobile-theme';
  const saved = localStorage.getItem(key);
  const preference = ref<Theme | null>(saved === 'light' || saved === 'dark' ? saved : null);
  const media = window.matchMedia('(prefers-color-scheme: dark)');
  const systemDark = ref(media.matches);
  const theme = computed<Theme>({
    get: () => preference.value ?? (systemDark.value ? 'dark' : 'light'),
    set(value) {
      preference.value = value;
      localStorage.setItem(key, value);
    },
  });
  function refreshSystemTheme() {
    if (!document.hidden) systemDark.value = media.matches;
  }
  media.addEventListener('change', refreshSystemTheme);
  document.addEventListener('visibilitychange', refreshSystemTheme);
  window.addEventListener('pageshow', refreshSystemTheme);
  onScopeDispose(() => {
    media.removeEventListener('change', refreshSystemTheme);
    document.removeEventListener('visibilitychange', refreshSystemTheme);
    window.removeEventListener('pageshow', refreshSystemTheme);
  });
  watch([theme, surface], ([value, current]) => syncBrowserChrome(value, current), { immediate: true });
  return theme;
}
