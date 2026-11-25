import { ref, shallowRef } from 'vue';
import type { Component } from 'vue';

export const markdownPreview = shallowRef<Component>();
export const markdownPreviewFailed = ref(false);
let loading: Promise<Component> | undefined;

export function preloadMarkdownPreview(): Promise<Component> {
  if (loading) return loading;
  markdownPreviewFailed.value = false;
  loading = Promise.all([
    import('md-editor-v3').then(({ MdPreview }) => MdPreview),
    import('md-editor-v3/lib/preview.css'),
  ]).then(([component]) => {
    markdownPreview.value = component;
    return component;
  }, failure => {
    markdownPreviewFailed.value = true;
    throw failure;
  });
  return loading;
}
