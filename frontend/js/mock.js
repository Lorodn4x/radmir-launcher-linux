const listeners = new Map();
const instantPreview = new URLSearchParams(window.location.search).has("instant");
const delay = (milliseconds) => new Promise((resolve) => setTimeout(resolve, instantPreview ? 0 : milliseconds));
const copy = (value) => JSON.parse(JSON.stringify(value));

let settings = {
  nickname: "Player_Name",
  selected_server_id: "4",
  auto_game_path: true,
  game_path_override: "",
  manifest_url: "https://launcher.invalid/manifest.json",
  download_base_url: "https://launcher.invalid/files/",
  portproton_path: "",
  auto_install_portproton: true,
  installer_sha256: "",
  download_concurrency: 4,
  hash_concurrency: 2,
  graphics_preset: "enhanced",
  wine_build: "PROTON_LG_10-28",
  graphics_api: "latest",
};

let portproton = {
  installed: true,
  kind: "system",
  version: "demo-1.0",
  message: "PortProton доступен (локальная демонстрация)",
};

let process = null;
let ready = false;

const bootstrap = () => ({
  settings: copy(settings),
  paths: {
    game_dir: "/home/demo/Games/RADMIR",
    config_dir: "/home/demo/.config/radmir-standalone",
    cache_dir: "/home/demo/.cache/radmir-standalone",
    state_dir: "/home/demo/.local/state/radmir-standalone",
  },
  portproton: copy(portproton),
  servers: [
    ...Array.from({length:21}, (_, i) => ({ id:String(i+1), name:`SERVER ${i+1}`, host:"demo.local", port:7777, online:true, players:200+i*30, capacity:1000, bonus:3 })),
  ],
  process: copy(process),
  preparation: { active: false, ready, files_ready: ready, environment: { launch_ready: ready }, process: copy(process),
    updates: {status: ready ? "current" : "not_installed", installed_build: ready ? "demo-2026.11.04" : null, latest_build:"demo-2026.11.04"} },
  news: [
    {
      id: "first-launch",
      title: "Первый запуск",
      summary: "Выберите сервер, проверьте никнейм и дождитесь готовности локальных файлов.",
      published_at: "2026-11-04T09:00:00Z",
      target_url: null,
      image_url: "./assets/graphics/2_advanced.svg",
      accent: "gold",
    },
    {
      id: "safe-update",
      title: "Контроль каждого файла",
      summary: "Launcher сравнивает контрольные суммы и скачивает только отсутствующие или изменённые данные.",
      published_at: "2026-11-03T12:00:00Z",
      target_url: null,
      accent: "cool",
    },
    {
      id: "portproton-setup",
      title: "Настройка среды",
      summary: "PortProton можно установить автоматически или указать путь вручную в расширенных параметрах.",
      published_at: "2026-11-02T15:00:00Z",
      target_url: null,
      accent: "mono",
    },
  ],
});

function emit(eventName, payload) {
  const eventListeners = listeners.get(eventName);
  if (!eventListeners) return;
  for (const handler of eventListeners) handler(copy(payload));
}

export async function mockListen(eventName, handler) {
  if (!listeners.has(eventName)) listeners.set(eventName, new Set());
  listeners.get(eventName).add(handler);
  return () => listeners.get(eventName)?.delete(handler);
}

async function simulateProgress(kind) {
  const filesTotal = kind === "verify" ? 24 : 18;
  const bytesTotal = kind === "verify" ? 0 : 734003200;
  emit("launcher://progress", {
    phase: "hashing",
    current_file: "data/core/index.json",
    bytes_done: 0,
    bytes_total: bytesTotal,
    files_done: 0,
    files_total: filesTotal,
    bytes_per_second: 0,
    percent: 0,
    message: "Проверяем локальные файлы (демо)",
  });
  await delay(450);

  for (let index = 1; index <= 4; index += 1) {
    emit("launcher://progress", {
      phase: "hashing",
      current_file: `data/core/chunk-${index}.dat`,
      bytes_done: 0,
      bytes_total: bytesTotal,
      files_done: Math.round((filesTotal * index) / 8),
      files_total: filesTotal,
      bytes_per_second: 0,
      percent: index * 5,
      message: "Сверяем SHA256 (демо)",
    });
    await delay(180);
  }

  if (kind !== "verify") {
    for (let index = 1; index <= 6; index += 1) {
      const percent = 20 + index * 12;
      emit("launcher://progress", {
        phase: "downloading",
        current_file: `client/assets/resource-${index}.pak`,
        bytes_done: Math.round((bytesTotal * index) / 6),
        bytes_total: bytesTotal,
        files_done: Math.round((filesTotal * index) / 6),
        files_total: filesTotal,
        bytes_per_second: 11800000 + index * 370000,
        percent,
        message: "Загружаем недостающие файлы (демо)",
      });
      await delay(260);
    }
  }

  emit("launcher://progress", {
    phase: "complete",
    current_file: "",
    bytes_done: bytesTotal,
    bytes_total: bytesTotal,
    files_done: filesTotal,
    files_total: filesTotal,
    bytes_per_second: 0,
    percent: 100,
    message: "Все файлы готовы (демо)",
  });
  await delay(200);
  emit("launcher://ready", { build: "demo-2026.11.04", ready: true });
}

export async function mockInvoke(command, args = {}) {
  if (command === "open_external" && typeof args.url === "string") {
    window.open(args.url, "_blank", "noopener,noreferrer");
    return null;
  }

  await delay(240);

  switch (command) {
    case "get_bootstrap":
      return copy(bootstrap());
    case "check_game_update":
      return {status: ready ? "current" : "not_installed", installed_build: ready ? "demo-2026.11.04" : null, latest_build:"demo-2026.11.04", message: ready ? "Актуальная демонстрационная сборка." : "Игра ещё не установлена."};
    case "save_settings":
      settings = { ...settings, ...copy(args.settings ?? {}) };
      return copy(settings);
    case "prepare_installation":
    case "prepare_game":
    case "verify_game": {
      if (!portproton.installed) throw new Error("PortProton не установлен. Установите среду запуска и повторите.");
      const kind = command === "verify_game" ? "verify" : "prepare";
      await simulateProgress(kind);
      ready = true;
      return {
        build: "demo-2026.11.04",
        ready: true,
        total_files: 24,
        valid_files: 24,
        missing_files: 0,
        changed_files: 0,
        downloaded_files: kind === "verify" ? undefined : 6,
        message: kind === "verify" ? "Все 24 файла прошли проверку (демо)" : "Сборка готова к запуску (демо)",
      };
    }
    case "check_portproton": return copy(portproton);
    case "list_runtime_versions": return [
      { id: "PROTON_LG_10-28", label: "PROTON_LG_10-28", installed: true, downloadable: true },
      { id: "PROTON_LG_9-27", label: "PROTON_LG_9-27", installed: false, downloadable: true },
    ];
    case "list_graphics_profiles": return [];
    case "get_dependency_plan": return { id: "demo", packages: [], enable_i386: false, changes: "", blocked: null };
    case "cancel_operation": return null;
    case "install_portproton": {
      for (const message of ["Проверяем пакеты (демо)", "Настраиваем prefix (демо)", "Завершаем установку (демо)"]) {
        emit("launcher://progress", {
          phase: "portproton",
          current_file: "",
          bytes_done: 0,
          bytes_total: 0,
          files_done: 0,
          files_total: 0,
          bytes_per_second: 0,
          percent: 0,
          message,
        });
        await delay(500);
      }
      portproton = {
        installed: true,
        kind: "bundled",
        version: "demo-1.0",
        message: "PortProton установлен (локальная демонстрация)",
      };
      return copy(portproton);
    }
    case "launch_game": {
      process = {
        id: "demo-process-1",
        pid: 4242,
        executable: "/demo/portproton/game.exe",
        server_id: args.serverId,
        status: "running",
        exit_code: null,
        log_path: "/home/demo/.local/state/radmir-standalone/game.log",
        started_at: new Date().toISOString(),
        ended_at: null,
      };
      return copy(process);
    }
    case "open_logs":
    case "minimize_window":
    case "toggle_maximize":
      return null;
    case "close_window":
      window.close();
      return null;
    default:
      throw new Error(`Демо backend не поддерживает команду: ${command}`);
  }
}
