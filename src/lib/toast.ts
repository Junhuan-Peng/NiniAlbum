import { writable } from 'svelte/store';

export interface ToastMessage { id: number; message: string; tone: 'default' | 'success' | 'error'; }
export const toasts = writable<ToastMessage[]>([]);
let nextId = 1;

export function toast(message: string, tone: ToastMessage['tone'] = 'default', duration = 3800) {
  const id = nextId++;
  toasts.update((items) => [...items, { id, message, tone }]);
  window.setTimeout(() => dismissToast(id), duration);
}

export function dismissToast(id: number) {
  toasts.update((items) => items.filter((item) => item.id !== id));
}

