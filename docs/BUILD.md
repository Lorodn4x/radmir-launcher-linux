# Сборка из исходников

Требуется Linux x86_64, актуальный стабильный Rust, Node.js 22 или новее,
Python 3, Clang и GNU binutils с поддержкой PE32.

## Ubuntu: зависимости разработки

```bash
sudo apt update
sudo apt install build-essential pkg-config curl wget file clang binutils \
  libssl-dev libgtk-3-dev libwebkit2gtk-4.1-dev \
  libayatana-appindicator3-dev librsvg2-dev \
  libgstreamer1.0-dev libgstreamer-plugins-base1.0-dev
```

Rust устанавливается через [rustup](https://rustup.rs/);
Node.js — через [официальные способы установки](https://nodejs.org/en/download).
Приведённый набор относится к окружению разработки, а не к списку пакетов,
которые пользователь должен устанавливать вручную перед запуском AppImage.

## Подготовка

В корне репозитория:

```bash
npm ci
npm run build:runner
```

Последняя команда создаёт runner.exe, command.exe и probe.exe из собственных
C-исходников. Они нужны перед компиляцией Rust и не хранятся в Git.

## Проверки

```bash
npm test
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

По умолчанию аппаратные и отдельные сетевые fixtures не запускаются.
Успех unit-тестов не подтверждает поддержку каждого GPU или ОС.

## Разработка

```bash
npm run dev
```

## AppImage и DEB

```bash
npm run build:appimage
npm run build:deb
```

Оба формата одним запуском:

```bash
npm run build:release
```

Результаты находятся в src-tauri/target/release/bundle/appimage/ и deb/.
GStreamer включается в AppImage средствами Tauri. Состав DEB проверяется
по metadata и зависимостям перед выпуском.

## Подготовка файлов релиза

```bash
python3 tools/stage-release.py \
  --bundle-dir src-tauri/target/release/bundle \
  --output release-assets
```

Скрипт собирает AppImage, DEB и SHA256SUMS из результатов сборки.
Выпуск на GitHub выполняется владельцем после проверки файлов.
CI проверяет исходники и не публикует релизы автоматически.

## Ресурсы

В репозитории используются собственные SVG-ресурсы и системные шрифты.
Изображения, звуки и шрифты оригинального лаунчера не требуются для сборки.
Игра, DirectX/VC++-установщики и PortProton загружаются приложением отдельно.
