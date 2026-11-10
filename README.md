# RustBlox

**EN** | [RU ниже](#rustblox-ru)

An open-source (**GPL-3.0**) third-party **Roblox client built in Rust**, using the official Roblox web APIs — the same endpoints the official website and official clients talk to. No protocol emulation, no private servers: everything goes to real Roblox infrastructure.

**Milestone 2 scope (this release):**
- **Everything from Milestone 1** (login + friends menu, see below)
- **Full GPU rewrite — wgpu + winit**: the entire UI (menu, login, HUD) is now rendered by wgpu pipelines (SDF rounded rects, glyph atlas, texture batching) instead of a CPU framebuffer; the 3D scene renderer shares the same device
- **Joining real places** — the actual Roblox game-join flow: `authentication-ticket/v1` → `gamejoin.roblox.com/v2/join-game` (v1 fallback) → the real game server (ip/port/jobId) → **RakNet** reliable-UDP transport (handshake, reliability, ACK/NACK, split reassembly, connected ping) implemented from the protocol spec
- **Replication stream parser** (tolerant, version-gated — see the honest status note below) feeding a runtime **DataModel**; the 3D renderer draws the world from it: **PBR-lite shaded parts, directional sun with a 2048px shadow map, hemispheric ambient, fog and a sky shader**, your own character rig from the real avatar appearance data, camera (free-fly, mouse-captured)
- Sign in to a Roblox account — password + 2-step verification, or a `.ROBLOSECURITY` session cookie
- Main menu styled after roblox.com home: a **collapsible left sidebar** (the rotated red logo tile doubles as the “R” of the wordmark; profile chip; Home / Add Friends navigation; icon-only mini mode), a **horizontal friends strip** with live statuses — **online / offline / what game they are playing right now** — and left/right scroll arrows (mouse wheel works too)
- **Places** search by name (`search-api/omni-search`) with a card grid that fills most of the screen — icon, name, creator, like ratio, player count and a green **Join** button that now starts the live join pipeline; while the search box is empty a personalized **recommendation feed** is shown (`discovery-api/omni-recommendation`)
- **Add Friends** pane: search users by username (`users.roblox.com/v1/users/search`) and send friend requests in place
- Automatic presence refresh every **10 seconds** (no manual refresh button)
- Session persistence
- Nightly automated builds for Windows / Linux / macOS published to the rolling [nightly release](../../releases/tag/nightly)
- Nothing more (on purpose — see [Roadmap](#roadmap))

> **Honest status of the live join (read this).** The HTTP join flow (auth ticket, gamejoin, server address), the RakNet transport and the renderer are fully real and tested against live servers (see `cargo run --example join_flow -- <placeId>` with `RUSTBLOX_COOKIE=...`). The layer on top of RakNet — Roblox's binary *replication protocol* — is proprietary and version-gated; its exact byte layout is being reverse-engineered. Until that converges, the connected session reports real diagnostics (ping, packets, decode stats) in the HUD, but the world may render empty. Every unknown packet is hex-logged (`RUSTBLOX_DEBUG=net`) to converge the parser. No fake data is ever rendered.

> **Disclaimer.** This project is not affiliated with, endorsed by, or connected to Roblox Corporation in any way. Roblox's Terms of Use may prohibit third-party clients; using one can put your account at risk. This client was written for educational purposes — use it at your own risk and never share your `.ROBLOSECURITY` token with anyone.

---

## Tech stack

| Layer     | Choice                                   | Why |
|-----------|------------------------------------------|-----|
| Windowing | [`winit 0.30`](https://crates.io/crates/winit) | The standard, tiny event loop |
| Rendering | [`wgpu 29`](https://crates.io/crates/wgpu) | The entire client is GPU-rendered: instanced SDF UI (rounded rects, rings, borders), a glyph-atlas text pass, image batching, plus the 3D scene pipeline (PBR-lite lighting, shadow map, sky, fog) |
| Text      | [`ab_glyph`](https://crates.io/crates/ab_glyph) | Glyphs are rasterized once into a shared GPU atlas and drawn as instanced quads; embedded DejaVu Sans (Latin + Cyrillic) |
| Game transport | own **RakNet** client over UDP | The reliable-UDP protocol Roblox game servers speak: offline handshake, reliability layer, fragmentation, ACKs, connected ping |
| HTTP      | [`ureq 2`](https://crates.io/crates/ureq) + rustls | No async runtime, no OpenSSL; pooled keep-alive connections, gzip |
| JSON      | `serde` / `serde_json`                   | — |

Every UI primitive records into a GPU command list (`draw.rs` keeps the exact same API as the old software rasterizer — a headless soft-renderer path is still used for the PNG previews and tests), then one instanced draw call per batch renders it. The place renderer (`gfx/scene.wgsl`) draws instanced boxes with sun shadows and a procedural sky from the same frame uniform buffer.

## Building

Rust 1.77+ (install via [rustup](https://rustup.rs)):

```bash
cargo build --release
# binary: target/release/rustblox
```

Works on **Windows, Linux and macOS** out of the box (TLS via rustls, no system dependencies). Rendering uses wgpu (Vulkan / Metal / DX12 / GL fallback).

### Nightly builds

Every night (02:00 UTC) and on demand (*Actions → Nightly builds → Run workflow*) a GitHub Actions pipeline runs the test suite, builds release binaries for **Linux x64 & ARM64, Windows x64 & ARM64, macOS Apple Silicon and macOS Intel**, and publishes them as zip archives to the rolling [`nightly`](../../releases/tag/nightly) pre-release (assets are replaced on each run).

### Mobile (experimental)

The [`mobile`](../../tree/mobile) branch builds for **Android (ARM64)** and **iOS (ARM64)** via *Actions → Mobile builds*. It publishes an **installable debug-signed APK**, an **unsigned iOS `.ipa`** (re-sign via AltStore/Sideloadly), plus the raw `.so`/`.a` libraries to the rolling [`mobile`](../../releases/tag/mobile) pre-release. Touch input and the system clipboard work on both platforms — details and known limitations in [docs/MOBILE.md](docs/MOBILE.md).

## Sign-in methods

### 1. Password (+ 2-step verification)
The same flow the official website uses:
1. `POST https://auth.roblox.com/v2/login` with `{"ctype":"Username","cvalue":"<login>","password":"..."}`
2. If the account has 2FA enabled, the response carries `twoStepVerificationData {ticket, mediaType}`; the code is verified via `POST /v2/twostepverification/verify` and the session cookie is returned.
3. If Roblox serves a **FunCaptcha** challenge for the login (happens often from "unusual" IPs, and always for some accounts), the client will tell you — use the cookie method instead. Solving Arkose captchas headlessly is out of scope for milestone 1.

### 2. Session cookie (`.ROBLOSECURITY`)
The most reliable method (also what most open-source tooling uses):
1. Log in to [roblox.com](https://www.roblox.com) in your browser
2. Press **F12** → **Application** (Firefox: **Storage**) → **Cookies** → `https://www.roblox.com`
3. Copy the value of **`.ROBLOSECURITY`** (it starts with `_|WARNING:-DO-NOT-SHARE-THIS...`)
4. Paste it into the *Session cookie* tab in RustBlox

The token is stored locally in your user config directory (`%APPDATA%/rustblox/session.json` on Windows, `~/.config/rustblox/session.json` on Linux/macOS) with owner-only permissions (0600 on Unix). It is never sent anywhere except `*.roblox.com` hosts.

## Networking details (how the official client does it)

- **CSRF rotation** — every mutating request carries `X-CSRF-Token`; a `403 Token Validation Failed` response includes a fresh `x-csrf-token` header *and* fresh anonymous cookies. RustBlox stores both (browser-style cookie jar) and retries, exactly like the website.
- **Batching** — friends: 1 request; presence: `POST /v1/presence/users` in chunks of ≤100 ids; game titles: 1 batched `GET /v1/games?universeIds=...`; avatars and place icons: 1 batched thumbnails request each. A full refresh of 200 friends is ~5 HTTP requests.
- **User search** — `GET /v1/users/search?keyword=...&limit=25` (Add Friends pane); friend requests go through `POST /v1/friends/{userId}/request-friendship` with the usual CSRF dance; API error messages (e.g. "Friend request already pending") are surfaced on the row.
- **Place discovery** — keyword search via `GET apis.roblox.com/search-api/omni-search`, default feed via `POST apis.roblox.com/discovery-api/omni-recommendation`; card icons via `GET /v1/places/gameicons` (batched).
- **Presence types** — `0` Offline, `1` Online (website), `2` InGame (with `universeId` → game title lookup, `gameId` = server job id, kept for the future "join game" milestone), `3` Studio, `4` Invisible.
- **Avatars** — decoded PNGs are cached on disk (keyed by thumbnail URL) and kept in GPU-uploadable RGBA8 form in memory; unchanged thumbnails are never re-downloaded.
- **Pinned DNS** — every API host (`auth/users/friends/presence/thumbnails/games/apis.roblox.com`) and the `tr/t0–t7.rbxcdn.com` thumbnail CDN resolve to hand-pinned IPs (`src/roblox/dns.rs`), verified by a full TLS + SNI handshake. API hosts never touch the system resolver at all, so DNS blocking/poisoning (e.g. RKN) cannot take the client down; the real hostname still goes into SNI and the `Host` header, so certificate validation and vhost routing are unaffected.
- **Session validation** — `GET /v1/users/authenticated`; a dead session drops you back to the login screen automatically.

## Architecture

```
src/
├── main.rs            # entry point
├── app.rs             # winit ApplicationHandler, screen state machine, input routing
├── login_ui.rs        # login screen (password / cookie tabs, 2FA dialog)
├── menu_ui.rs         # main menu: collapsible sidebar, horizontal friends strip, places search + card grid, add-friends pane
├── ui.rs              # widgets: text fields (IME!), buttons, hit-testing
├── draw.rs            # dual-mode primitives: GPU command recorder + headless soft rasterizer
├── font.rs            # ab_glyph → shared glyph atlas (GPU quads) / direct raster (soft path)
├── theme.rs           # colors / metrics
├── model.rs           # domain models, sorting, humanized "last online"
├── config.rs          # session persistence (0600)
├── avatar_cache.rs    # disk cache for avatar bitmaps
├── net.rs             # background worker thread + avatar pool → EventLoopProxy
├── raknet.rs          # RakNet client: handshake, reliability, ACKs, splits, ping
├── gfx/               # the wgpu renderer
│   ├── mod.rs         # device/surface/pipelines, font atlas, image cache, frame loop
│   ├── ui.wgsl        # instanced SDF UI shader (rects, rounded rects, rings, glyphs, images)
│   ├── scene.wgsl     # 3D: instanced parts, sun shadow map, fog; sky gradient shader
│   ├── scene.rs       # SceneView/Camera/Environment data model
│   └── math.rs        # dependency-free column-major mat4/vec3
├── world/             # runtime DataModel (instance tree + properties) → SceneView builder
├── game/              # live session: join state machine, replication thread, camera controller
└── roblox/
    ├── client.rs      # ureq agent, cookie jar, CSRF rotation
    ├── auth.rs        # login / 2FA verify / resend / logout
    ├── social.rs      # friends, presence, games, thumbnails (batched)
    ├── join.rs        # LIVE JOIN: authentication-ticket → gamejoin v2/v1 → server addr + avatar
    ├── replication.rs # tolerant replication stream parser → DataModel events
    ├── types.rs       # serde models of the API
    ├── dns.rs         # pinned Roblox/CDN IPs (DNS-blocking-proof resolution)
    └── errors.rs      # typed errors with readable messages
```

### Joining a place (how it works)

1. Click **Join** on a place card (or `cargo run --example join_flow -- <placeId>` headlessly).
2. `POST apis.roblox.com/authentication-ticket/v1` exchanges your session for a one-time ticket.
3. `POST gamejoin.roblox.com/v2/join-game` (v1 fallback) returns the real **ip:port** of a live game server + `jobId`.
4. A full **RakNet handshake** connects the UDP socket to that server (open-connection 1/2, connection request/accepted, NEW_INCOMING_CONNECTION, connected pings).
5. Every replication payload is parsed into the DataModel; the renderer rebuilds the scene from it a few times a second. The HUD shows real connection diagnostics.
6. `Esc` leaves the session (disconnection notification sent).

Debug logging: `RUSTBLOX_DEBUG=net game gfx cargo run`.

The UI thread never blocks: all network work runs on a worker thread (plus a 4-thread avatar pool) that talks back through `winit`'s `EventLoopProxy`, waking the event loop only when data arrives.

## Optimization notes

- Release profile: `lto = "fat"`, `codegen-units = 1`, `opt-level = 3`, `panic = "abort"`, symbols stripped.
- No async runtime at all — one worker thread is plenty for an IO-bound menu client, and startup is instant.
- HTTP/1.1 keep-alive connection reuse via a single `ureq` agent; gzip everywhere.
- On-demand redraws (no busy render loop): the event loop sleeps until the next animation tick (spinner/caret/countdown) or the next scheduled refresh.
- Only visible list rows are rasterized per frame; avatars only decoded once per URL version.
- Presence, games and thumbnails requests are batched in ≤100-id chunks per API limits.

## Roadmap

- **M1:** login + Roblox-style friends menu (sidebar, add-friends search) with presence ✔
- **M2 — this release:** full wgpu rewrite + real join flow (auth ticket → gamejoin → RakNet UDP) + replication parser (reverse-engineering stage) + 3D renderer (PBR-lite, shadows, sky) + character rig from avatar data ✔
- **M3:** converge the replication protocol on live captures (full world streaming, other players, physics), chat, notifications, Charts / Avatar pages
- **M4:** full place rendering parity (meshes/decals/terrain streaming, scripts-lite)

## License

**GPL-3.0-only** — see [LICENSE](LICENSE).
Embedded fonts: DejaVu Sans (free license, see `assets/fonts/LICENSE-DejaVu.txt`).

---
---

# RustBlox (RU)

Open-source (**GPL-3.0**) сторонний **клиент Roblox на Rust**, работающий с официальными веб-API Roblox — теми же эндпоинтами, что используют официальный сайт и официальные клиенты. Никакой «своей реализации» серверов: всё ходит в реальную инфраструктуру Roblox.

**Задание №2 (эта версия):**
- **Всё из Milestone 1** (вход + меню друзей, см. ниже)
- **Полный переход на GPU — wgpu + winit**: весь интерфейс (меню, логин, HUD) рисуется wgpu-пайплайнами (SDF-скругления, атлас глифов, батчинг текстур) вместо CPU-фреймбуфера; 3D-рендер мира работает на том же устройстве
- **Подключение к реальным плейсам** — настоящий игровой join-flow Roblox: `authentication-ticket/v1` → `gamejoin.roblox.com/v2/join-game` (фолбэк v1) → реальный игровой сервер (ip/port/jobId) → транспорт **RakNet** (надёжный UDP: хэндшейк, reliability, ACK/NACK, сборка фрагментов, ping) по спецификации протокола
- **Парсер потока репликации** (толерантный, версия-зависимый — см. честное примечание ниже), наполняющий рантайм-**DataModel**; 3D-рендер рисует мир из неё: **парты с PBR-упрощённым освещением, направленное солнце с теневой картой 2048px, полусферический ambient, туман и шейдер неба**, ваш персонаж из реальных данных аватара, свободная камера с захватом мыши
- Вход в аккаунт Roblox — пароль + двухэтапная верификация либо cookie-сессия `.ROBLOSECURITY`
- Главное меню в стиле домашней страницы roblox.com: **складной левый сайдбар** (повёрнутая красная плитка-логотип служит буквой «R» в названии; чип профиля; навигация Home / Add Friends; мини-режим только с иконками), **горизонтальная полоса друзей** со статусами — **в сети / не в сети / во что сейчас играет** — и стрелками прокрутки влево/вправо (колесо мыши тоже работает)
- **Поиск Places** по названию (`search-api/omni-search`) с сеткой карточек — иконка, название, автор, рейтинг, игроки и зелёная кнопка **Join**, запускающая реальный join-пайплайн; при пустом поиске — персональная лента рекомендаций (`discovery-api/omni-recommendation`)
- Панель **«Добавить друзей»**: поиск пользователей по нику (`users.roblox.com/v1/users/search`) и отправка заявок в друзья прямо из приложения
- Автообновление статусов каждые **10 секунд** (без ручной кнопки обновления)
- Сохранение сессии
- Ночные автоматические сборки для Windows / Linux / macOS публикуются в скользящем [nightly-релизе](../../releases/tag/nightly)
- И ничего больше (осознанно — см. [Roadmap](#roadmap))

> **Честный статус live-входа (прочитайте).** HTTP-цепочка входа (auth ticket, gamejoin, адрес сервера), транспорт RakNet и рендерер — полностью реальные и протестированы против живых серверов (см. `cargo run --example join_flow -- <placeId>` с `RUSTBLOX_COOKIE=...`). Слой поверх RakNet — бинарный *протокол репликации* Roblox — проприетарный и зависящий от версии клиента; его точная байтовая раскладка в процессе реверс-инжиниринга. Пока он не сойдётся, подключённая сессия показывает реальные диагностические данные (ping, пакеты, статистика декодирования) в HUD, но мир может отрисоваться пустым. Каждый неизвестный пакет дампится в hex (`RUSTBLOX_DEBUG=net`) для дожима парсера. Фейковые данные не рисуются никогда.

> **Важно.** Проект не связан с Roblox Corporation. Условия использования Roblox могут запрещать сторонние клиенты — используйте на свой риск. Никогда не передавайте свой `.ROBLOSECURITY` никому (включая этот клиент — он хранит токен только локально).

## Сборка

Нужен Rust 1.77+ ([rustup](https://rustup.rs)):

```bash
cargo build --release
# бинарник: target/release/rustblox
```

Работает на **Windows, Linux и macOS** без системных зависимостей (TLS — rustls).

### Ночные сборки

Каждую ночь (02:00 UTC) и по кнопке (*Actions → Nightly builds → Run workflow*) GitHub Actions прогоняет тесты, собирает release-бинарники для **Linux x64 & ARM64, Windows x64 & ARM64, macOS Apple Silicon и macOS Intel** и публикует их zip-архивами в скользящий пререлиз [`nightly`](../../releases/tag/nightly) (ассеты перезаписываются при каждой сборке).

### Мобильные (экспериментально)

Ветка [`mobile`](../../tree/mobile) собирается для **Android (ARM64)** и **iOS (ARM64)** через *Actions → Mobile builds*. Публикуются **устанавливаемый APK** (отладочная подпись), **неподписанный iOS-`.ipa`** (переподписать через AltStore/Sideloadly) и сырые библиотеки `.so`/`.a` — в скользящий пререлиз [`mobile`](../../releases/tag/mobile). Тач-ввод и системный буфер обмена работают на обеих платформах — подробности и ограничения в [docs/MOBILE.md](docs/MOBILE.md).

## Как войти

**Пароль (+2FA):** тот же флоу, что у официального сайта (`auth.roblox.com/v2/login` → при 2FA ввод кода → verify). Если Roblox показывает капчу FunCaptcha (часто бывает с «необычных» IP), клиент предложит использовать cookie-метод.

**Cookie `.ROBLOSECURITY` (надёжный способ):** зайдите на roblox.com в браузере → **F12** → **Application** → **Cookies** → `https://www.roblox.com` → скопируйте значение **`.ROBLOSECURITY`** → вставьте во вкладку «Session cookie». Токен хранится локально (`~/.config/rustblox/session.json` / `%APPDATA%\rustblox\session.json`, права 0600 на Unix) и отправляется только на `*.roblox.com`.

## Что внутри (кратко)

- **Стек:** `winit` (окно) + **`wgpu` 29** (весь UI — инстансные SDF-примитивы + атлас глифов; 3D-мир — PBR-упрощённое освещение, теневая карта, небо, туман) + `ab_glyph` (атлас текста) + `ureq`/rustls (HTTP) + собственный **RakNet**-клиент (UDP-транспорт игровых серверов).
- **Как у официального клиента:** ротация `X-CSRF-Token` (403 → свежий токен из ответа + куки → повтор), cookie-jar, батчинг (presence ≤100 id за запрос, игры/аватарки/иконки плейсов пачками), статусы 0/1/2/3/4, `gameId` сервера уже сохраняется для будущего входа в игру.
- **Поиск плейсов:** `GET apis.roblox.com/search-api/omni-search` по ключевому слову, дефолтная лента — `POST apis.roblox.com/discovery-api/omni-recommendation` (с авторизацией, как на сайте); иконки карточек — `GET /v1/places/gameicons` пачками.
- **Как работает вход в плейс:** карточка **Join** → `authentication-ticket/v1` → `gamejoin/v2` (ip:port + jobId) → полный RakNet-хэндшейк → парсинг репликации в DataModel → сцена перерисовывается из мира несколько раз в секунду; HUD показывает живые диагностики. `Esc` — выход (disconnection notification). Дебаг-логи: `RUSTBLOX_DEBUG=net game gfx cargo run`.
- **Оптимизации:** LTO fat + codegen-units=1, инстансная отрисовка (один draw call на батч UI и на весь мир), атлас глифов и кэш текстур аватарок, дисковый кэш аватарок, ~5 HTTP-запросов на полный рефреш 200 друзей.

## Дорожная карта

1. **M1:** вход + меню в стиле Roblox (складной сайдбар, полоса друзей, поиск и карточки плейсов, заявки в друзья) со статусами ✔
2. **M2 (текущая):** полный переход на wgpu + реальный join-flow (auth ticket → gamejoin → RakNet UDP) + парсер репликации (стадия реверс-инжиниринга) + 3D-рендер (PBR-lite, тени, небо) + риг персонажа из данных аватара ✔
3. **M3:** дожим протокола репликации на живых дамках (полный стриминг мира, другие игроки, физика), чат, уведомления, страницы Charts / Avatar
4. **M4:** полный паритет рендеринга плейса (меши/декали/стриминг терраина)

## Лицензия

**GPL-3.0-only** — см. [LICENSE](LICENSE). Шрифты DejaVu — свободная лицензия (`assets/fonts/LICENSE-DejaVu.txt`).
