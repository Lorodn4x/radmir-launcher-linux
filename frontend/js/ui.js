import { isDemo } from "./api.js";
import { canPlay, mainAction, runtimeLocked, selectedServer, state } from "./state.js";
import { clamp, formatBytes, formatDate, formatPlayers, formatSpeed, isValidNickname, phaseLabel, processIsRunning } from "./format.js";

export const $ = (selector) => document.querySelector(selector);
const $$ = (selector) => [...document.querySelectorAll(selector)];

let previousServerSignature = "";
let previousNewsSignature = "";

function setValue(selector, value) {
  const element = $(selector);
  if (document.activeElement !== element) element.value = value ?? "";
}

function setText(selector, value) {
  $(selector).textContent = value ?? "";
}

function renderNavigation() {
  const settingsVisible = state.view === "settings";
  $("#page-home").hidden = settingsVisible;
  $("#page-settings").hidden = !settingsVisible;
  $(".servers").hidden = settingsVisible;
  $("#sidebar-settings").hidden = !settingsVisible;
  $(".online-total").hidden = settingsVisible;
  $("#app").classList.toggle("settings-visible", settingsVisible);
  const video = $("#news-video");
  if (settingsVisible) video.pause();
  else if (!video.hidden && video.paused) video.play().catch(() => {});
  for (const button of $$("[data-view]")) {
    const active = button.dataset.view === state.view;
    button.classList.toggle("is-active", active && button.classList.contains("rail-button"));
    if (button.classList.contains("rail-button")) {
      if (active) button.setAttribute("aria-current", "page");
      else button.removeAttribute("aria-current");
    }
  }
}

function renderServers() {
  const signature = JSON.stringify([state.servers, state.settings?.selected_server_id, state.bootStatus, state.favourites]);
  if (signature === previousServerSignature) return;
  previousServerSignature = signature;

  const list = $("#server-list");
  list.replaceChildren();
  list.hidden = state.bootStatus !== "ready" || state.servers.length === 0;
  $("#servers-empty").hidden = state.bootStatus !== "ready" || state.servers.length > 0;
  setText("#server-count", state.bootStatus === "ready" ? String(state.servers.length) : "—");

  if (state.bootStatus === "loading") {
    for (let index = 0; index < 3; index += 1) {
      const item = document.createElement("li");
      item.className = "server-skeleton skeleton";
      item.setAttribute("aria-hidden", "true");
      list.append(item);
    }
    return;
  }

  const sorted = [...state.servers].sort((a, b) => Number(state.favourites.includes(b.id)) - Number(state.favourites.includes(a.id)));
  let section = "";
  for (const server of sorted) {
    const heading = state.favourites.includes(server.id) ? "Избранные" : "Рекомендуемые";
    if (section !== heading) {
      section = heading;
      const label = document.createElement("li");
      label.className = "server-section";
      label.textContent = heading;
      list.append(label);
    }
    const item = document.createElement("li");
    item.className = "server-list__item";
    const button = document.createElement("button");
    const selected = server.id === state.settings?.selected_server_id;
    button.type = "button";
    button.className = `server-button${selected ? " is-selected" : ""}`;
    button.dataset.serverId = server.id;
    button.setAttribute("aria-pressed", String(selected));
    button.setAttribute("aria-label", `${server.name}, ${formatPlayers(server, isDemo)}`);

    const dot = document.createElement("span");
    dot.className = `online-dot${server.online ? " is-online" : ""}`;
    dot.setAttribute("aria-hidden", "true");

    const name = document.createElement("span");
    name.className = "server-button__name";
    name.textContent = server.id;
    const meta = document.createElement("span");
    meta.className = "server-button__meta";
    meta.textContent = formatPlayers(server, isDemo);

    const host = document.createElement("button");
    host.type = "button";
    host.className = "server-button__host";
    host.textContent = state.favourites.includes(server.id) ? "★" : "☆";
    host.dataset.favourite = server.id;
    host.setAttribute("aria-label", `Избранный сервер ${server.id}`);
    host.setAttribute("aria-pressed", String(state.favourites.includes(server.id)));
    button.append(name, dot, meta);
    item.append(button, host);
    list.append(item);
  }
  const hasOnline = state.servers.some(s => s.players != null);
  setText("#online-total", hasOnline ? state.servers.reduce((n, s) => n + (s.players || 0), 0).toLocaleString("ru-RU") : "—");
}

function renderNews() {
  const signature = JSON.stringify([state.news, state.newsIndex]);
  if (signature === previousNewsSignature) return;
  previousNewsSignature = signature;

  const article = state.news[state.newsIndex] ?? {
    id: "empty",
    title: "Новости недоступны",
    summary: "Не удалось получить ленту RADMIR. Подготовка игры продолжается.",
    published_at: null,
    accent: "gold",
  };
  setText("#news-title", article.title);
  setText("#news-summary", article.summary);
  setText("#news-date", article.published_at ? formatDate(article.published_at) : "Инструкция");
  $("#news-date").dateTime = article.published_at ?? "";
  $("#news-art").dataset.accent = article.accent || "gold";
  const image = $("#news-image");
  const video = $("#news-video");
  image.hidden = !article.image_url;
  if (article.image_url) image.src = article.image_url;
  else image.removeAttribute("src");
  video.pause();
  video.hidden = !article.video_url;
  if (article.video_url) {
    video.src = article.video_url;
    video.load();
    if (state.view === "home") video.play().catch(() => { video.hidden = true; });
  } else if (video.hasAttribute("src")) { video.removeAttribute("src"); video.load(); }
  $(".media-volume").hidden = !article.video_url;
  video.onerror = () => { video.hidden = true; };
  $("#news-art").hidden = Boolean(article.image_url);
  $("#news-more").disabled = state.news.length === 0;
  $("#news-previous").disabled = state.news.length < 2;
  $("#news-next").disabled = state.news.length < 2;

  const dots = $("#news-dots");
  dots.replaceChildren();
  state.news.forEach((item, index) => {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `slider-dot${index === state.newsIndex ? " is-active" : ""}`;
    button.dataset.newsIndex = String(index);
    button.setAttribute("aria-label", `Новость ${index + 1}: ${item.title}`);
    button.setAttribute("aria-pressed", String(index === state.newsIndex));
    dots.append(button);
  });
}

function renderProgress() {
  const busy = ["preparing", "hashing", "downloading", "verifying", "installing"].includes(state.prepareStatus);
  const progress = state.progress;
  const showProgress = busy && Boolean(progress);
  $("#progress-card").hidden = !showProgress;

  if (showProgress) {
    const percent = clamp(progress.percent);
    const indeterminate = !progress.bytes_total && !progress.files_total;
    setText("#progress-phase", phaseLabel(progress.phase));
    setText("#progress-percent", indeterminate ? "…" : `${Math.round(percent)}%`);
    $("#progress-bar").hidden = indeterminate;
    $("#progress-indeterminate").hidden = !indeterminate;
    $("#progress-bar").value = percent;
    $("#progress-bar").setAttribute("aria-valuetext", progress.message || phaseLabel(progress.phase));
    setText("#progress-file", progress.current_file || progress.message || "Пожалуйста, подождите…");
    setText("#progress-bytes", Number(progress.bytes_total) > 0
      ? `${formatBytes(progress.bytes_done)} / ${formatBytes(progress.bytes_total)} · осталось ${formatBytes(Math.max(0, progress.bytes_total - progress.bytes_done))}`
      : "—");
    setText("#progress-files", Number(progress.files_total) > 0
      ? `${Number(progress.files_done) || 0} / ${Number(progress.files_total) || 0}`
      : "—");
    setText("#progress-speed", formatSpeed(progress.bytes_per_second));
  }

  const result = state.prepareResult;
  const hasResultDetails = result && !busy && (result.build || result.total_files !== undefined);
  $("#result-details").hidden = !hasResultDetails;
  if (hasResultDetails) {
    setText("#result-build", `Сборка: ${result.build || state.build || "—"}`);
    const details = [`Файлы: ${result.valid_files ?? 0} / ${result.total_files ?? 0}`];
    if (result.missing_files) details.push(`отсутствует: ${result.missing_files}`);
    if (result.changed_files) details.push(`изменено: ${result.changed_files}`);
    if (result.downloaded_files !== undefined) details.push(`загружено: ${result.downloaded_files}`);
    setText("#result-files", details.join(" · "));
  }
}

function renderGameState() {
  const running = processIsRunning(state.process);
  const busy = ["preparing", "hashing", "downloading", "verifying", "installing"].includes(state.prepareStatus);
  const light = $("#game-status-light");
  light.className = "status-light";
  if (running || (state.ready && !state.error)) light.classList.add("is-ready");
  else if (busy || state.launching) light.classList.add("is-busy");
  else if (state.error || state.prepareStatus === "error") light.classList.add("is-error");

  let label = "Подготовка игры";
  let message = "Ожидание данных…";
  if (state.prepareStatus === "installing") {
    label = "Настройка PortProton";
    message = state.progress?.message || "Устанавливаем среду запуска…";
  } else if (busy) {
    label = phaseLabel(state.progress?.phase || "preparing");
    message = state.progress?.message || "Это может занять несколько минут.";
  } else if (state.launching) {
    label = "Запуск игры";
    message = "Передаём параметры выбранного сервера…";
  } else if (running) {
    label = state.process.status === "running" ? "Игра запущена" : state.process.status === "recovering" ? "Восстанавливаем наблюдение" : "Запуск игры";
    message = state.process.status === "running" ? "Игра подтвердила запуск. Можно закрыть окно лаунчера." : "Ожидаем подтверждения helper; повторный запуск заблокирован.";
  } else if (state.updates?.status === "update_available") {
    label = "Есть обновление";
    message = state.updates.message;
  } else if (state.ready) {
    label = "Готово к игре";
    message = state.updates?.message || "Сохранённая подготовка действительна.";
  } else if (state.prepareStatus === "error") {
    label = "Нужно внимание";
    message = state.prepareResult?.message || state.error?.message || "Не удалось подготовить файлы.";
  }
  setText("#game-state-label", label);
  setText("#game-status-message", message);

  $("#build-chip").hidden = !state.build;
  setText("#build-chip", state.build);
  setText("#build-label", state.build ? `/ ${state.build}` : "");

  const playable = canPlay();
  const action = mainAction();
  $("#play-button").disabled = !action.enabled;
  setText("#play-label", action.label);
  const bonus = selectedServer()?.bonus;
  $("#bonus-badge").hidden = !bonus || bonus < 2;
  setText("#bonus-badge", `x${bonus} АКЦИЯ`);
  let hint = "Сначала дождитесь готовности файлов";
  if (state.launching) hint = "Запускаем процесс…";
  else if (running) hint = "Игровой процесс уже запущен";
  else if (action.kind === "update") hint = `Загрузить версию ${state.updates.latest_build || "игры"}`;
  else if (action.kind === "download") hint = "Установить игру и подготовить среду";
  else if (action.kind === "prepare") hint = "Проверить и восстановить установку";
  else if (!isValidNickname(state.settings?.nickname)) hint = "Введите корректный никнейм";
  else if (!selectedServer()) hint = "Выберите сервер в списке слева";
  else if (!state.ready) hint = "Проверка или загрузка ещё не завершена";
  else if (playable) hint = `${selectedServer().name} · ${state.settings.nickname}`;
  setText("#play-hint", hint);

  $("#install-portproton").hidden = !state.portproton || state.portproton.installed || state.environment?.runtime_ready || busy || state.operationActive;
  $("#verify-button").disabled = busy || running || state.launching || state.bootStatus !== "ready";

  const process = state.process;
  $("#process-card").hidden = !process;
  if (process) {
    const failed = process.status === "failed";
    $("#process-card").classList.toggle("is-error", failed);
    setText("#process-label", process.status === "running" ? "Игра запущена" : process.status === "starting" ? "Запуск игры" : process.status === "recovering" ? "Восстановление наблюдения" : failed ? "Ошибка завершения" : "Процесс завершён");
    const details = [`PID ${process.pid ?? "—"}`, process.status || "unknown"];
    if (process.exit_code !== null && process.exit_code !== undefined) details.push(`код ${process.exit_code}`);
    setText("#process-detail", details.join(" · "));
    $("#logs-button").disabled = !process.log_path;
  }
  renderProgress();
}

function renderPortProton() {
  const status = state.portproton;
  const light = $("#portproton-light");
  light.className = "status-light";
  if (state.environment?.runtime_ready) light.classList.add("is-ready");
  else if (state.prepareStatus === "installing") light.classList.add("is-busy");
  else light.classList.add("is-error");
  const version = status?.version ? ` · ${status.version}` : "";
  const kind = status?.kind ? ` (${status.kind})` : "";
  setText("#portproton-message", state.environment?.runtime_ready
    ? `PortProton готов · ${state.environment.wine_build}`
    : status ? `${status.message || (status.installed ? "Установлен" : "Не установлен")}${version}${kind}` : "Проверяем установку…");
}

function renderRuntime() {
  const locked = runtimeLocked();
  const wine = $("#wine-build");
  const choices = [...state.runtimeVersions];
  const selected = state.settings?.wine_build || "PROTON_LG_10-28";
  if (!choices.some(item => item.id === selected)) choices.unshift({ id: selected, label: selected, installed: false, downloadable: true });
  const signature = JSON.stringify(choices);
  if (wine.dataset.signature !== signature) {
    wine.replaceChildren(...choices.map(item => {
      const option = document.createElement("option");
      option.value = item.id;
      option.textContent = `${item.label} · ${item.installed ? "установлена" : item.downloadable ? "доступна загрузка" : "недоступна"}`;
      option.disabled = !item.installed && !item.downloadable;
      return option;
    }));
    wine.dataset.signature = signature;
  }
  setValue("#wine-build", selected);
  const api = $("#graphics-api");
  const profiles = state.graphicsProfiles.length ? state.graphicsProfiles : [
    { id: "latest", label: "DXVK/VKD3D — новейшие", dxvk_version: "2.7.1-509", vkd3d_version: "1.1-5122", minimum_vulkan: "1.3" },
    { id: "stable", label: "DXVK/VKD3D — стабильные", dxvk_version: "2.6.2", vkd3d_version: "2.14.1", minimum_vulkan: "1.3" },
    { id: "sarek", label: "DXVK/VKD3D Sarek", dxvk_version: "sarek-1.11.0", vkd3d_version: "sarek-2.6.0", minimum_vulkan: "1.1" },
    { id: "opengl", label: "WineD3D / OpenGL" },
  ];
  const apiSignature = JSON.stringify(profiles);
  if (api.dataset.signature !== apiSignature) {
    api.replaceChildren(...profiles.map(profile => {
      const option = document.createElement("option");
      option.value = profile.id;
      option.textContent = profile.label;
      return option;
    }));
    api.dataset.signature = apiSignature;
  }
  setValue("#graphics-api", state.settings?.graphics_api || "latest");
  const profile = profiles.find(item => item.id === api.value);
  setText("#graphics-api-hint", profile?.dxvk_version
    ? `DXVK ${profile.dxvk_version} · VKD3D ${profile.vkd3d_version} · Vulkan ${profile.minimum_vulkan} и новее. Совместимость проверяется в выбранном Wine.`
    : "Использует OpenGL. Совместимость проверяется в выбранном Wine.");
  setText("#runtime-environment", state.environment?.message || "Среда будет проверена перед запуском.");
  const suggested = profiles.find(item => item.id === state.environment?.suggested_graphics_api);
  $("#suggested-api").hidden = !suggested;
  $("#suggested-api").disabled = locked;
  setText("#suggested-api", suggested ? `Проверить ${suggested.label}` : "");
  for (const selector of ["#wine-build", "#graphics-api", "#portproton-path", "#auto-install-portproton", "#auto-game-path", "#manifest-url", "#download-base-url", "#download-concurrency", "#hash-concurrency", "#game-path", "#prepare-installation"]) $(selector).disabled = locked;
  $("#dependency-plan-button").disabled = locked || state.dependencyLoading;
  const plan = state.dependencyPlan;
  $("#dependency-plan").hidden = !plan;
  if (plan) {
    setText("#dependency-summary", plan.blocked || (plan.packages.length ? `Нужны пакеты: ${plan.packages.join(", ")}` : "Все проверенные системные зависимости установлены."));
    setText("#dependency-changes", plan.changes);
    $("#install-dependencies").hidden = !plan.packages.length;
    $("#install-dependencies").disabled = locked || state.dependencyLoading || Boolean(plan.blocked);
    setText("#install-dependencies", plan.enable_i386 ? "Включить i386 и рассчитать полный план" : "Установить с подтверждением администратора");
  }
}

function renderSettings() {
  const settings = state.settings;
  if (!settings) return;
  setValue("#nickname", settings.nickname);
  $("#nickname").disabled = false;
  const locked = state.launching || processIsRunning(state.process) || ["preparing", "hashing", "downloading", "verifying", "installing"].includes(state.prepareStatus);
  for (const selector of ["#auto-game-path", "#game-path", "#manifest-url", "#download-base-url"]) $(selector).disabled = locked;
  const nicknameValid = isValidNickname(settings.nickname);
  $("#nickname").classList.toggle("is-valid", nicknameValid);
  $("#profile-status").className = `profile__status${settings.nickname ? nicknameValid ? " is-valid" : " is-invalid" : ""}`;

  $("#auto-game-path").checked = Boolean(settings.auto_game_path);
  const resolvedPath = settings.auto_game_path
    ? state.paths?.game_dir || ""
    : settings.game_path_override || "";
  setValue("#game-path", resolvedPath);
  $("#game-path").readOnly = Boolean(settings.auto_game_path);
  setText("#resolved-path-hint", settings.auto_game_path
    ? `Автоматический путь: ${state.paths?.game_dir || "задаёт backend"}`
    : "Путь можно изменить. Он сохраняется при снятии фокуса.");

  setValue("#manifest-url", settings.manifest_url);
  setValue("#download-base-url", settings.download_base_url);
  setValue("#portproton-path", settings.portproton_path);
  setValue("#installer-sha256", settings.installer_sha256);
  $("#auto-install-portproton").checked = Boolean(settings.auto_install_portproton);
  setValue("#download-concurrency", settings.download_concurrency);
  setValue("#hash-concurrency", settings.hash_concurrency);

  for (const button of $$("[data-graphics]")) {
    const selected = button.dataset.graphics === settings.graphics_preset;
    button.classList.toggle("is-selected", selected);
    button.setAttribute("aria-pressed", String(selected));
  }
  setText("#save-status", state.saving ? "Сохраняем…" : state.saveStatus || "");
}

function renderError() {
  $("#global-error").hidden = !state.error;
  if (!state.error) return;
  const message = state.error.message || "Неизвестная ошибка";
  setText("#global-error-message", state.error.log_path ? `${message} · Журнал: ${state.error.log_path}` : message);
  $("#global-retry").hidden = state.error.retryable === false;
}

export function render() {
  const loaded = state.bootStatus === "ready";
  $("#app").setAttribute("aria-busy", String(!loaded));
  $("#boot-screen").hidden = loaded;
  $("#loaded-content").hidden = !loaded;
  $("#demo-badge").hidden = !isDemo;
  renderNavigation();
  renderServers();
  renderNews();
  renderSettings();
  renderPortProton();
  renderRuntime();
  renderGameState();
  renderError();
}

export function showToast(message) {
  clearTimeout(state.toastTimer);
  const toast = $("#toast");
  toast.textContent = message;
  toast.hidden = false;
  state.toastTimer = setTimeout(() => {
    toast.hidden = true;
  }, 3200);
}

export function showInfo({ eyebrow = "RADMIR / ИНФО", title, paragraphs = [], steps = [] }) {
  setText("#dialog-eyebrow", eyebrow);
  setText("#dialog-title", title);
  const body = $("#dialog-body");
  body.replaceChildren();
  for (const text of paragraphs) {
    const paragraph = document.createElement("p");
    paragraph.textContent = text;
    body.append(paragraph);
  }
  if (steps.length) {
    const list = document.createElement("ol");
    for (const text of steps) {
      const item = document.createElement("li");
      item.textContent = text;
      list.append(item);
    }
    body.append(list);
  }
  $("#info-dialog").showModal();
}
