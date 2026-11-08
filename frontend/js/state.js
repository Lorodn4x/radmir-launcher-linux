export const state = {
  favourites: [],
  bootStatus: "loading",
  view: "home",
  settings: null,
  paths: null,
  servers: [],
  portproton: null,
  process: null,
  environment: null,
  operationActive: false,
  operationId: null,
  runtimeVersions: [],
  graphicsProfiles: [],
  dependencyPlan: null,
  dependencyLoading: false,
  news: [],
  newsIndex: 0,
  build: "",
  ready: false,
  updates: { status: "checking", installed_build: null, latest_build: null, message: "Проверяем версию…" },
  updateChecking: false,
  prepareStatus: "idle",
  prepareResult: null,
  progress: null,
  launching: false,
  saving: false,
  error: null,
  errorContext: "bootstrap",
  toastTimer: null,
};

export const defaultSettings = {
  nickname: "",
  selected_server_id: "",
  auto_game_path: true,
  game_path_override: "",
  manifest_url: "",
  download_base_url: "",
  portproton_path: "",
  auto_install_portproton: true,
  installer_sha256: "",
  download_concurrency: 4,
  hash_concurrency: 2,
  graphics_preset: "standard",
  wine_build: "PROTON_LG_10-28",
  graphics_api: "latest",
};

export function selectedServer() {
  return state.servers.find((server) => server.id === state.settings?.selected_server_id) ?? null;
}

export function canPlay() {
  return Boolean(
    state.ready
      && state.prepareStatus === "ready"
      && selectedServer()
      && isValid(state.settings?.nickname)
      && !state.launching
      && !state.operationActive
      && !state.saving
      && state.updates?.status !== "update_available"
      && state.process?.status !== "running"
      && state.process?.status !== "starting"
      && state.process?.status !== "recovering",
  );
}

export function runtimeLocked() {
  return state.operationActive || state.launching || state.saving
    || ["preparing", "hashing", "downloading", "verifying", "installing"].includes(state.prepareStatus)
    || ["starting", "running", "recovering"].includes(state.process?.status);
}

export function applyPreparation(snapshot) {
  state.operationActive = Boolean(snapshot.active);
  state.operationId = snapshot.operation_id ?? null;
  state.environment = snapshot.environment ?? null;
  state.ready = Boolean(snapshot.ready && snapshot.environment?.launch_ready);
  state.process = snapshot.process ?? null;
  if (snapshot.updates) state.updates = snapshot.updates;
  state.build = state.updates?.installed_build || "";
  state.progress = snapshot.active ? snapshot.progress : null;
  const error = snapshot.error || (!snapshot.active && !state.ready ? snapshot.environment?.graphics_error : null);
  state.prepareStatus = snapshot.active ? "preparing" : state.ready ? "ready" : error ? "error" : "idle";
  if (error) state.error = error;
  else if (snapshot.active || state.ready) state.error = null;
}

export function mainAction() {
  if (state.launching) return { kind: "play", label: "ЗАПУСК…", enabled: false };
  if (["starting", "running", "recovering"].includes(state.process?.status)) return { kind: "play", label: "В ИГРЕ", enabled: false };
  if (runtimeLocked()) return { kind: "prepare", label: "ПОДГОТОВКА…", enabled: false };
  const enabled = state.bootStatus === "ready" && !state.dependencyLoading;
  if (state.updates?.status === "update_available") return { kind: "update", label: "ОБНОВИТЬ", enabled };
  if (!state.updates?.installed_build) return { kind: "download", label: "ЗАГРУЗИТЬ", enabled };
  if (!state.ready) return { kind: "prepare", label: "ПОДГОТОВИТЬ", enabled };
  return { kind: "play", label: "ИГРАТЬ", enabled: canPlay() };
}

export function shouldAutoPrepare() {
  return state.bootStatus === "ready" && !state.ready && !state.error && !runtimeLocked()
    && !["checking", "update_available"].includes(state.updates?.status);
}
export function needsDependencyPlan(error) {
  return ["missing_dependencies", "missing_32bit_graphics", "graphics_probe_failed"].includes(error?.code);
}

function isValid(value) {
  return /^[A-Za-z_]{3,20}$/.test(value ?? "");
}
