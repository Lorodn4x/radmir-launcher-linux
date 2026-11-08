import { mockInvoke, mockListen } from "./mock.js";

const hasTauri = Boolean(
  window.__TAURI__?.core?.invoke
  && window.__TAURI__?.event?.listen,
);

export const isDemo = !hasTauri;

export function invoke(command, args = {}) {
  if (hasTauri) return window.__TAURI__.core.invoke(command, args);
  return mockInvoke(command, args);
}

export function listen(eventName, handler) {
  if (hasTauri) return window.__TAURI__.event.listen(eventName, (event) => handler(event.payload));
  return mockListen(eventName, handler);
}
