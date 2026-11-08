import { invoke, isDemo, listen } from "./api.js";
import { $, render, showInfo, showToast } from "./ui.js";
import { applyPreparation, canPlay, defaultSettings, mainAction, needsDependencyPlan, runtimeLocked, shouldAutoPrepare, state } from "./state.js";

let saveQueue = Promise.resolve();
let pendingSaves = 0;

function errorPayload(error, retryable = true) {
  return {
    message: error instanceof Error ? error.message : String(error?.message || error || "Неизвестная ошибка"),
    retryable: error?.retryable ?? retryable,
    code: error?.code ?? "operation_failed",
    log_path: error?.log_path ?? null,
  };
}

function clearError() {
  state.error = null;
}

function saveSettings(patch) {
  const previous = { ...state.settings };
  Object.assign(state.settings, patch);
  const requested = { ...state.settings };
  state.saving = true;
  state.saveStatus = "";
  pendingSaves += 1;
  render();

  const operation = async () => {
    try {
      const saved = await invoke("save_settings", { settings: requested });
      if (pendingSaves === 1) state.settings = { ...defaultSettings, ...saved };
      state.saveStatus = "Сохранено";
      return true;
    } catch (error) {
      if (pendingSaves === 1) state.settings = previous;
      state.saveStatus = "Не сохранено";
      state.error = errorPayload(error, false);
      showToast("Не удалось сохранить настройки");
      return false;
    } finally {
      pendingSaves -= 1;
      state.saving = pendingSaves > 0;
      render();
    }
  };
  saveQueue = saveQueue.then(operation, operation);
  return saveQueue;
}

async function refreshRuntimeOptions() {
  const results = await Promise.allSettled([invoke("list_runtime_versions"), invoke("list_graphics_profiles")]);
  if (results[0].status === "fulfilled") state.runtimeVersions = results[0].value;
  if (results[1].status === "fulfilled") state.graphicsProfiles = results[1].value;
  render();
}

async function loadDependencyPlan() {
  state.dependencyLoading = true;
  render();
  try {
    state.dependencyPlan = await invoke("get_dependency_plan");
  } catch (error) { state.error = errorPayload(error); }
  finally { state.dependencyLoading = false; render(); }
}

async function prepareGame(command = "prepare_installation") {
  if (runtimeLocked()) return;
  state.prepareStatus = "preparing";
  state.progress = null;
  state.ready = false;
  state.prepareResult = null;
  clearError();
  render();
  try {
    const result = await invoke(command);
    state.prepareResult = result;
    state.build = result.build || state.build;
    state.ready = Boolean(result.ready);
    state.prepareStatus = result.ready ? "ready" : "error";
    if (!result.ready) {
      state.error = errorPayload(result.message || "Подготовка не завершена");
      state.errorContext = command;
    }
    state.portproton = await invoke("check_portproton");
  } catch (error) {
    state.ready = false;
    state.prepareStatus = "error";
    state.error = errorPayload(error);
    state.errorContext = command;
    if (needsDependencyPlan(error)) {
      state.view = "settings";
      document.querySelector("details.advanced").open = true;
      await loadDependencyPlan();
    }
  } finally {
    state.operationActive = false;
    state.progress = null;
    render();
    await refreshRuntimeOptions();
    if (state.ready) await refreshUpdateStatus();
  }
}

async function continuePreparation() { await prepareGame(); }

async function refreshUpdateStatus() {
  if (state.updateChecking) return;
  state.updateChecking = true;
  try { state.updates = await invoke("check_game_update"); }
  catch { state.updates = { ...state.updates, status: state.updates?.status === "update_available" ? "update_available" : "unavailable", message: "Не удалось проверить обновления. Повторим позже." }; }
  finally { state.updateChecking = false; render(); }
}

async function bootstrap() {
  state.bootStatus = "loading";
  state.ready = false;
  state.prepareStatus = "idle";
  state.progress = null;
  state.prepareResult = null;
  clearError();
  state.errorContext = "bootstrap";
  render();
  try {
    const data = await invoke("get_bootstrap");
    state.settings = { ...defaultSettings, ...data.settings };
    state.paths = data.paths ?? null;
    state.servers = Array.isArray(data.servers) ? data.servers : [];
    state.portproton = data.portproton ?? { installed: false, message: "Статус PortProton не получен" };
    state.process = data.process ?? null;
    if (data.preparation) applyPreparation(data.preparation);
    else state.environment = data.environment ?? null;
    state.news = Array.isArray(data.news) ? data.news : [];
    state.newsIndex = 0;
    if (!state.settings.selected_server_id && state.servers.length > 0) {
      state.settings.selected_server_id = state.servers[0].id;
      await saveSettings({ selected_server_id: state.servers[0].id });
    }
    state.bootStatus = "ready";
    render();
    refreshRuntimeOptions();
    // Warm readiness renders immediately; version metadata is fetched quietly.
    // An available update is an action for the user, never an automatic download.
    await refreshUpdateStatus();
    if (shouldAutoPrepare()) await continuePreparation();
  } catch (error) {
    state.bootStatus = "error";
    state.error = errorPayload(error);
    state.errorContext = "bootstrap";
    render();
  }
}

async function launchGame() {
  if (!canPlay()) {
    showToast("Проверьте никнейм, сервер и готовность файлов");
    return;
  }
  state.launching = true;
  clearError();
  render();
  try {
    state.process = await invoke("launch_game", {
      serverId: state.settings.selected_server_id,
      nickname: state.settings.nickname,
    });
    showToast(state.process?.status === "running" ? "Игра запущена" : "Ожидаем подтверждения запуска игры…");
  } catch (error) {
    state.error = errorPayload(error, false);
    showToast("Не удалось запустить игру");
  } finally {
    state.launching = false;
    render();
  }
}

function moveNews(offset) {
  if (state.news.length < 2) return;
  state.newsIndex = (state.newsIndex + offset + state.news.length) % state.news.length;
  render();
}

function openCurrentNews() {
  const article = state.news[state.newsIndex];
  if (!article) return;
  if (article.target_url) {
    invoke("open_external", { url: article.target_url }).catch(error => showToast(errorPayload(error).message));
    return;
  }
  const guides = {
    "first-launch": [
      "Укажите никнейм длиной от 3 до 20 символов: латинские буквы и подчёркивание.",
      "Выберите доступный сервер в панели слева.",
      "Дождитесь проверки файлов и нажмите «Играть».",
    ],
    "safe-update": [
      "Проверка файлов сравнивает локальные данные с manifest и контрольными суммами.",
      "Можно закрыть окно: загрузка продолжится в фоне. Повторное открытие покажет текущий прогресс.",
      "Если проверка завершилась ошибкой, повторите её кнопкой в настройках.",
    ],
    "portproton-setup": [
      "Автоустановка включается в расширенных параметрах.",
      "Для ручной настройки укажите путь к PortProton и отключите автоустановку.",
      "Все загружаемые компоненты проверяются перед установкой.",
    ],
  };
  showInfo({
    title: article.title,
    paragraphs: [article.summary],
    steps: guides[article.id] ?? ["Следуйте инструкциям на экране и проверяйте статус подготовки перед запуском."],
  });
}

function bindEvents() {
  try { state.favourites = JSON.parse(localStorage.getItem("radmir-favourites") || "[]").filter(id => typeof id === "string"); } catch { state.favourites = []; }
  $("#sidebar-settings").append($(".settings-column"));
  const persistField = async (event) => {
    const input = event.target;
    if (!input.name || !state.settings || !input.reportValidity() || runtimeLocked()) return;
    const value = input.type === "checkbox" ? input.checked : input.value;
    const patch = { [input.name]: value };
    if (input.name === "auto_game_path" && !value && !state.settings.game_path_override) patch.game_path_override = state.paths?.game_dir || "";
    state.ready = false;
    state.prepareStatus = "idle";
    if (await saveSettings(patch)) await continuePreparation();
  };
  $("#sidebar-settings").addEventListener("change", persistField);
  for (let i = 0; i < 7; i++) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = i === 2 ? "is-selected" : "";
    button.setAttribute("aria-label", `Сцена ${i + 1}`);
    button.setAttribute("aria-pressed", String(i === 2));
    const image = document.createElement("img");
    image.src = `./assets/graphics/${i}_default.svg`;
    image.alt = "";
    button.append(image);
    button.onclick = () => {
      $("#graphics-default").src = `./assets/graphics/${i}_default.svg`;
      $("#graphics-advanced").src = `./assets/graphics/${i}_advanced.svg`;
      for (const thumb of $("#graphics-thumbnails").children) {
        thumb.classList.toggle("is-selected", thumb === button);
        thumb.setAttribute("aria-pressed", String(thumb === button));
      }
    };
    $("#graphics-thumbnails").append(button);
  }
  $("#graphics-split").oninput = event => $("#graphics-comparison").style.setProperty("--split", `${event.target.value}%`);
  $("#news-volume").oninput = event => {
    const video = $("#news-video");
    video.volume = Number(event.target.value) / 100;
    video.muted = video.volume === 0;
    if (!video.hidden) video.play().catch(() => {});
  };
  document.addEventListener("click", (event) => {
    const favourite = event.target.closest("[data-favourite]");
    if (favourite) {
      const id = favourite.dataset.favourite;
      state.favourites = state.favourites.includes(id) ? state.favourites.filter(v => v !== id) : [...state.favourites, id];
      try { localStorage.setItem("radmir-favourites", JSON.stringify(state.favourites)); } catch {}
      render();
      return;
    }
    const viewButton = event.target.closest("[data-view]");
    if (viewButton) {
      state.view = viewButton.dataset.view;
      $(".main__content").scrollTop = 0;
      render();
    }

    const serverButton = event.target.closest("[data-server-id]");
    if (serverButton && serverButton.dataset.serverId !== state.settings?.selected_server_id) {
      saveSettings({ selected_server_id: serverButton.dataset.serverId });
    }

    const newsDot = event.target.closest("[data-news-index]");
    if (newsDot) {
      state.newsIndex = Number(newsDot.dataset.newsIndex);
      render();
    }

    const graphicsButton = event.target.closest("[data-graphics]");
    if (graphicsButton && graphicsButton.dataset.graphics !== state.settings?.graphics_preset) {
      saveSettings({ graphics_preset: graphicsButton.dataset.graphics });
    }

    const socialButton = event.target.closest("[data-url]");
    if (socialButton) {
      invoke("open_external", { url: socialButton.dataset.url })
        .catch((error) => showToast(errorPayload(error, false).message));
    }
  });

  $("#nickname").addEventListener("input", (event) => {
    if (!state.settings) return;
    state.settings.nickname = event.target.value.trim();
    state.saveStatus = "";
    render();
  });
  $("#nickname").addEventListener("change", (event) => {
    event.target.value = event.target.value.trim();
    if (event.target.reportValidity()) saveSettings({ nickname: event.target.value });
    else showToast("Никнейм: 3–20 латинских букв или подчёркиваний");
  });

  $("#settings-form").addEventListener("submit", (event) => event.preventDefault());
  $("#settings-form").addEventListener("change", async (event) => {
    const input = event.target;
    if (!input.name || !state.settings || !input.reportValidity() || runtimeLocked()) return;
    let value = input.type === "checkbox" ? input.checked : input.value;
    if (input.type === "number") value = Number(value);
    if (input.name === "game_path_override" && state.settings.auto_game_path) return;
    const patch = { [input.name]: value };
    if (input.name === "auto_game_path" && !value && !state.settings.game_path_override) {
      patch.game_path_override = state.paths?.game_dir || "";
    }
    if (["manifest_url", "download_base_url"].includes(input.name)) {
      state.ready = false;
      state.prepareStatus = "idle";
    }
    if (await saveSettings(patch)) {
      if (["wine_build", "graphics_api", "portproton_path", "manifest_url", "download_base_url", "auto_install_portproton"].includes(input.name)) {
        state.ready = false;
        state.prepareStatus = "idle";
        await continuePreparation();
      }
    }
  });

  for (const [selector, command] of [
    ["#minimize-button", "minimize_window"],
    ["#maximize-button", "toggle_maximize"],
    ["#close-button", "close_window"],
  ]) {
    $(selector).addEventListener("click", async () => {
      try {
        await invoke(command);
        if (isDemo && command !== "close_window") showToast("Команда окна показана в демо-режиме");
      } catch (error) {
        showToast(errorPayload(error, false).message);
      }
    });
  }

  $("#news-previous").addEventListener("click", () => moveNews(-1));
  $("#news-next").addEventListener("click", () => moveNews(1));
  $("#news-more").addEventListener("click", openCurrentNews);
  $("#news-carousel").addEventListener("keydown", (event) => {
    if (event.key === "ArrowLeft") {
      event.preventDefault();
      moveNews(-1);
    } else if (event.key === "ArrowRight") {
      event.preventDefault();
      moveNews(1);
    }
  });

  $("#play-button").addEventListener("click", () => {
    const action = mainAction();
    if (!action.enabled) return;
    if (action.kind === "play") launchGame();
    else continuePreparation();
  });
  $("#verify-button").addEventListener("click", () => prepareGame("verify_game"));
  $("#install-portproton").addEventListener("click", continuePreparation);
  $("#prepare-installation").addEventListener("click", continuePreparation);
  $("#cancel-operation").addEventListener("click", () => invoke("cancel_operation").catch(error => showToast(errorPayload(error).message)));
  $("#dependency-plan-button").addEventListener("click", loadDependencyPlan);
  $("#install-dependencies").addEventListener("click", async () => {
    if (!state.dependencyPlan || runtimeLocked()) return;
    state.dependencyLoading = true;
    render();
    try {
      state.dependencyPlan = await invoke("install_system_dependencies", { planId: state.dependencyPlan.id });
      if (!state.dependencyPlan.packages.length) {
        state.prepareStatus = "idle";
        state.operationActive = false;
        await continuePreparation();
      }
    } catch (error) { state.error = errorPayload(error); await loadDependencyPlan(); }
    finally { state.dependencyLoading = false; render(); }
  });
  $("#suggested-api").addEventListener("click", async () => {
    const api = state.environment?.suggested_graphics_api;
    if (api && !runtimeLocked() && await saveSettings({ graphics_api: api })) {
      state.ready = false;
      state.prepareStatus = "idle";
      await continuePreparation();
    }
  });
  $("#logs-button").addEventListener("click", async () => {
    try {
      await invoke("open_logs");
    } catch (error) {
      showToast(errorPayload(error, false).message);
    }
  });

  $("#global-retry").addEventListener("click", async () => {
    if (state.errorContext === "bootstrap") await bootstrap();
    else if (state.errorContext === "portproton") await continuePreparation();
    else if (state.errorContext === "exit") await launchGame();
    else await prepareGame("prepare_game");
  });
  $("#error-dismiss").addEventListener("click", () => {
    clearError();
    render();
  });

  $("#forum-button").onclick = () => invoke("open_external", { url:"https://forum.radmir.games" }).catch(error => showToast(errorPayload(error).message));
  $("#support-button").onclick = () => invoke("open_external", { url:"https://vk.com/radmirsup" }).catch(error => showToast(errorPayload(error).message));
}

async function registerEvents() {
  await Promise.all([
    listen("worker://status", (snapshot) => {
      if (state.bootStatus !== "ready") return;
      applyPreparation(snapshot);
      if (!state.saving && snapshot.settings) {
        state.settings.wine_build = snapshot.settings.wine_build;
        state.settings.graphics_api = snapshot.settings.graphics_api;
        state.settings.portproton_path = snapshot.settings.portproton_path;
      }
      if (!snapshot.active && needsDependencyPlan(state.error) && !state.dependencyPlan && !state.dependencyLoading) {
        state.view = "settings";
        document.querySelector("details.advanced").open = true;
        loadDependencyPlan();
      }
      render();
    }),
    listen("game://status", (payload) => {
      const justStarted = state.process?.status !== "running" && payload.status === "running";
      state.process = payload;
      if (justStarted) showToast("Игра запущена");
      render();
    }),
    listen("launcher://progress", (payload) => {
      state.progress = payload;
      state.prepareStatus = payload.phase === "portproton" || payload.phase === "installing" || payload.phase?.startsWith("setup-")
        ? "installing"
        : payload.phase === "hashing"
          ? "hashing"
          : payload.phase === "downloading"
            ? "downloading"
            : "preparing";
      clearError();
      render();
    }),
    listen("launcher://ready", (payload) => {
      state.build = payload.build || state.build;
      state.ready = Boolean(payload.ready);
      state.prepareStatus = payload.ready ? "ready" : "error";
      state.progress = null;
      render();
    }),
    listen("launcher://error", (payload) => {
      state.error = errorPayload(payload, payload.retryable !== false);
      state.errorContext = "prepare_game";
      if (state.bootStatus === "ready") {
        state.ready = false;
        state.prepareStatus = "error";
        state.progress = null;
      }
      render();
    }),
    listen("game://exit", (payload) => {
      state.process = payload;
      state.launching = false;
      if (payload.status === "failed") {
        state.error = errorPayload(`Не удалось завершить игровую сессию штатно (код ${payload.exit_code ?? "не получен"}). Откройте логи для диагностики.`);
        state.errorContext = "exit";
      }
      render();
    }),
  ]);
}

async function init() {
  const requestedView = new URLSearchParams(window.location.search).get("view");
  if (["home", "settings"].includes(requestedView)) state.view = requestedView;
  bindEvents();
  render();
  try {
    await registerEvents();
  } catch (error) {
    showToast(`События backend недоступны: ${errorPayload(error, false).message}`);
  }
  await bootstrap();
  const refreshServers = async () => {
    try {
      if (!document.hidden && state.bootStatus === "ready") {
        state.servers = await invoke("list_servers");
        render();
      }
    } catch {} finally { setTimeout(refreshServers, 10000); }
  };
  setTimeout(refreshServers, 10000);
  setInterval(() => { if (!document.hidden && state.bootStatus === "ready") refreshUpdateStatus(); }, 300000);
  setInterval(() => { if (!document.hidden && state.view === "home" && !$("#news-carousel").matches(":hover, :focus-within")) moveNews(1); }, 15000);
}

init();
