const numberFormat = new Intl.NumberFormat("ru-RU");
const dateFormat = new Intl.DateTimeFormat("ru-RU", {
  day: "2-digit",
  month: "long",
  year: "numeric",
});

export function clamp(value, minimum = 0, maximum = 100) {
  const number = Number(value);
  return Number.isFinite(number) ? Math.min(maximum, Math.max(minimum, number)) : minimum;
}

export function isValidNickname(value) {
  return /^[A-Za-z_]{3,20}$/.test(value ?? "");
}

export function formatBytes(value) {
  const bytes = Number(value);
  if (!Number.isFinite(bytes) || bytes < 0) return "—";
  if (bytes === 0) return "0 Б";
  const units = ["Б", "КиБ", "МиБ", "ГиБ", "ТиБ"];
  const unitIndex = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  const amount = bytes / (1024 ** unitIndex);
  return `${amount.toLocaleString("ru-RU", { maximumFractionDigits: unitIndex > 1 ? 1 : 0 })} ${units[unitIndex]}`;
}

export function formatSpeed(value) {
  const speed = Number(value);
  return Number.isFinite(speed) && speed > 0 ? `${formatBytes(speed)}/с` : "—";
}

export function formatDate(value) {
  if (/^\d{2}\.\d{2}\.\d{4}$/.test(value)) return value.replaceAll(".", " / ");
  const date = new Date(value);
  return Number.isNaN(date.getTime()) ? "Без даты" : dateFormat.format(date);
}

export function formatPlayers(server, demo) {
  const capacity = Number(server.capacity) > 0 ? Number(server.capacity) : 1000;
  if (server.players === null || server.players === undefined) return `— / ${capacity}`;
  if (server.online === false) return "Оффлайн";
  const players = numberFormat.format(Math.max(0, Number(server.players) || 0));
  return `${players} / ${numberFormat.format(capacity)}${demo ? " · демо" : ""}`;
}

export function phaseLabel(phase) {
  const labels = {
    preflight: "Проверка системы",
    "runtime-install": "Подготовка PortProton",
    "runtime-download": "Загрузка компонентов PortProton",
    "runtime-extract": "Распаковка компонентов",
    "prefix-backup": "Резервная копия префикса",
    "prefix-init": "Подготовка Wine-префикса",
    "game-files": "Проверка игры",
    "game-libraries": "Установка игровых библиотек",
    "graphics-check": "Проверка 32-битной графики",
    "system-dependencies": "Установка системных пакетов",
    hashing: "Проверка контрольных сумм",
    downloading: "Загрузка файлов",
    extracting: "Распаковка файлов",
    verifying: "Проверка файлов",
    portproton: "Установка PortProton",
    installing: "Установка PortProton",
    preparing: "Подготовка файлов",
    manifest: "Получение manifest",
    "setup-download": "Загрузка PortProton",
    "setup-install": "Установка PortProton",
    ready: "Подготовка завершена",
    complete: "Подготовка завершена",
  };
  return labels[phase] ?? "Подготовка игры";
}

export function processIsRunning(process) {
  return ["running", "starting", "recovering"].includes(process?.status);
}
