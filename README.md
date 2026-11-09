# RustBlox

**EN** | [RU ниже](#rustblox-ru)

An open-source (**GPL-3.0**) third-party **Roblox client built in Rust**, using the official Roblox web APIs — the same endpoints the official website and official clients talk to. No protocol emulation, no private servers: everything goes to real Roblox infrastructure.

**Milestone 1 scope (this release):**
- Sign in to a Roblox account — password + 2-step verification, or a `.ROBLOSECURITY` session cookie
- Main menu styled after roblox.com home: a **collapsible left sidebar** (the rotated red logo tile doubles as the “R” of the wordmark; profile chip; Home / Add Friends navigation; icon-only mini mode), a **horizontal friends strip** with live statuses — **online / offline / what game they are playing right now** — and left/right scroll arrows (mouse wheel works too)
- **Places** search by name (`search-api/omni-search`) with a card grid that fills most of the screen — icon, name, creator, like ratio, player count and a green **Join** button (joining itself is a Milestone 2 feature); while the search box is empty a personalized **recommendation feed** is shown (`discovery-api/omni-recommendation`)
- **Add Friends** pane: search users by username (`users.roblox.com/v1/users/search`) and send friend requests in place
- Automatic presence refresh every **10 seconds** (no manual refresh button)
- Session persistence
- Nightly automated builds for Windows / Linux / macOS published to the rolling [nightly release](../../releases/tag/nightly)
- Nothing more (on purpose — see [Roadmap](#roadmap))

> **Disclaimer.** This project is not affiliated with, endorsed by, or connected to Roblox Corporation in any way. Roblox's Terms of Use may prohibit third-party clients; using one can put your account at risk. This client was written for educational purposes — use it at your own risk and never share your `.ROBLOSECURITY` token with anyone.

---

## Tech stack

| Layer     | Choice                                   | Why |
|-----------|------------------------------------------|-----|
| Windowing | [`winit 0.30`](https://crates.io/crates/winit) | The standard, tiny event loop |
| Rendering | [`pixels 0.17`](https://crates.io/crates/pixels) (wgpu) | A raw GPU-backed pixel framebuffer — minimal overhead now, and a clean path to full 3D place rendering on the same wgpu device later |
| Text      | [`ab_glyph`](https://crates.io/crates/ab_glyph) | Fast on-the-fly glyph rasterization, embedded DejaVu Sans (Latin + Cyrillic) |
| HTTP      | [`ureq 2`](https://crates.io/crates/ureq) + rustls | No async runtime, no OpenSSL; pooled keep-alive connections, gzip |
| JSON      | `serde` / `serde_json`                   | — |

The UI is drawn software-side into an RGBA8 buffer (rects, glyphs, avatars) and blitted by wgpu — a deliberate "own the pixels" design that keeps the binary small and the frame budget predictable, and later allows the Roblox place renderer to share the same wgpu surface.

## Building

Rust 1.77+ (install via [rustup](https://rustup.rs)):

```bash
cargo build --release
# binary: target/release/rustblox
```

Works on **Windows, Linux and macOS** out of the box (TLS via rustls, no system dependencies). Rendering uses wgpu (Vulkan / Metal / DX12 / GL fallback).

### Nightly builds

Every night (02:00 UTC) and on demand (*Actions → Nightly builds → Run workflow*) a GitHub Actions pipeline runs the test suite, builds release binaries for **Linux x64, Windows x64, macOS Apple Silicon and macOS Intel**, and publishes them as zip archives to the rolling [`nightly`](../../releases/tag/nightly) pre-release (assets are replaced on each run).

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
- **Session validation** — `GET /v1/users/authenticated`; a dead session drops you back to the login screen automatically.

## Architecture

```
src/
├── main.rs            # entry point
├── app.rs             # winit ApplicationHandler, screen state machine, input routing
├── login_ui.rs        # login screen (password / cookie tabs, 2FA dialog)
├── menu_ui.rs         # main menu: collapsible sidebar, horizontal friends strip, places search + card grid, add-friends pane
├── ui.rs              # widgets: text fields (IME!), buttons, hit-testing
├── draw.rs            # software rasterizer over the pixels framebuffer
├── font.rs            # ab_glyph text rendering
├── theme.rs           # colors / metrics
├── model.rs           # domain models, sorting, humanized "last online"
├── config.rs          # session persistence (0600)
├── avatar_cache.rs    # disk cache for avatar bitmaps
├── net.rs             # background worker thread + avatar pool → EventLoopProxy
└── roblox/
    ├── client.rs      # ureq agent, cookie jar, CSRF rotation
    ├── auth.rs        # login / 2FA verify / resend / logout
    ├── social.rs      # friends, presence, games, thumbnails (batched)
    ├── types.rs       # serde models of the API
    └── errors.rs      # typed errors with readable messages
```

The UI thread never blocks: all network work runs on a worker thread (plus a 4-thread avatar pool) that talks back through `winit`'s `EventLoopProxy`, waking the event loop only when data arrives.

## Optimization notes

- Release profile: `lto = "fat"`, `codegen-units = 1`, `opt-level = 3`, `panic = "abort"`, symbols stripped.
- No async runtime at all — one worker thread is plenty for an IO-bound menu client, and startup is instant.
- HTTP/1.1 keep-alive connection reuse via a single `ureq` agent; gzip everywhere.
- On-demand redraws (no busy render loop): the event loop sleeps until the next animation tick (spinner/caret/countdown) or the next scheduled refresh.
- Only visible list rows are rasterized per frame; avatars only decoded once per URL version.
- Presence, games and thumbnails requests are batched in ≤100-id chunks per API limits.

## Roadmap

- **M1 — this release:** login + Roblox-style friends menu (sidebar, add-friends search) with presence ✔
- **M2:** joining real game servers (the official RakNet-based UDP transport, `gameId` is already captured by the presence API) and rendering places with wgpu on the same window/surface stack — the reserved *Places* strip will host the place cards
- **M3:** chat, notifications, Charts / Avatar pages (the disabled sidebar entries)
- **M4:** full place rendering pipeline (instances, replication, input)

## License

**GPL-3.0-only** — see [LICENSE](LICENSE).
Embedded fonts: DejaVu Sans (free license, see `assets/fonts/LICENSE-DejaVu.txt`).

---
---

# RustBlox (RU)

Open-source (**GPL-3.0**) сторонний **клиент Roblox на Rust**, работающий с официальными веб-API Roblox — теми же эндпоинтами, что используют официальный сайт и официальные клиенты. Никакой «своей реализации» серверов: всё ходит в реальную инфраструктуру Roblox.

**Задание №1 (эта версия):**
- Вход в аккаунт Roblox — пароль + двухэтапная верификация либо cookie-сессия `.ROBLOSECURITY`
- Главное меню в стиле домашней страницы roblox.com: **складной левый сайдбар** (повёрнутая красная плитка-логотип служит буквой «R» в названии; чип профиля; навигация Home / Add Friends; мини-режим только с иконками), **горизонтальная полоса друзей** со статусами — **в сети / не в сети / во что сейчас играет** — и стрелками прокрутки влево/вправо (колесо мыши тоже работает)
- **Поиск Places** по названию (`search-api/omni-search`) с сеткой карточек на большую часть экрана — иконка, название, автор, рейтинг (лайки), число игроков и зелёная кнопка **Join** (само подключение — в Milestone 2); при пустом поиске показывается персональная лента рекомендаций (`discovery-api/omni-recommendation`)
- Панель **«Добавить друзей»**: поиск пользователей по нику (`users.roblox.com/v1/users/search`) и отправка заявок в друзья прямо из приложения
- Автообновление статусов каждые **10 секунд** (без ручной кнопки обновления)
- Сохранение сессии
- Ночные автоматические сборки для Windows / Linux / macOS публикуются в скользящем [nightly-релизе](../../releases/tag/nightly)
- И ничего больше (осознанно — см. [Roadmap](#roadmap))

> **Важно.** Проект не связан с Roblox Corporation. Условия использования Roblox могут запрещать сторонние клиенты — используйте на свой риск. Никогда не передавайте свой `.ROBLOSECURITY` никому (включая этот клиент — он хранит токен только локально).

## Сборка

Нужен Rust 1.77+ ([rustup](https://rustup.rs)):

```bash
cargo build --release
# бинарник: target/release/rustblox
```

Работает на **Windows, Linux и macOS** без системных зависимостей (TLS — rustls).

### Ночные сборки

Каждую ночь (02:00 UTC) и по кнопке (*Actions → Nightly builds → Run workflow*) GitHub Actions прогоняет тесты, собирает release-бинарники для **Linux x64, Windows x64, macOS Apple Silicon и macOS Intel** и публикует их zip-архивами в скользящий пререлиз [`nightly`](../../releases/tag/nightly) (ассеты перезаписываются при каждой сборке).

## Как войти

**Пароль (+2FA):** тот же флоу, что у официального сайта (`auth.roblox.com/v2/login` → при 2FA ввод кода → verify). Если Roblox показывает капчу FunCaptcha (часто бывает с «необычных» IP), клиент предложит использовать cookie-метод.

**Cookie `.ROBLOSECURITY` (надёжный способ):** зайдите на roblox.com в браузере → **F12** → **Application** → **Cookies** → `https://www.roblox.com` → скопируйте значение **`.ROBLOSECURITY`** → вставьте во вкладку «Session cookie». Токен хранится локально (`~/.config/rustblox/session.json` / `%APPDATA%\rustblox\session.json`, права 0600 на Unix) и отправляется только на `*.roblox.com`.

## Что внутри (кратко)

- **Стек:** `winit` (окно) + `pixels`/wgpu (пиксельный фреймбуфер — с прицелом на будущий 3D-рендер плейсов на том же wgpu) + `ab_glyph` (текст) + `ureq`/rustls (сеть, без tokio и OpenSSL).
- **Как у официального клиента:** ротация `X-CSRF-Token` (403 → свежий токен из ответа + куки → повтор), cookie-jar, батчинг (presence ≤100 id за запрос, игры/аватарки/иконки плейсов пачками), статусы 0/1/2/3/4, `gameId` сервера уже сохраняется для будущего входа в игру.
- **Поиск плейсов:** `GET apis.roblox.com/search-api/omni-search` по ключевому слову, дефолтная лента — `POST apis.roblox.com/discovery-api/omni-recommendation` (с авторизацией, как на сайте); иконки карточек — `GET /v1/places/gameicons` пачками.
- **Оптимизации:** LTO fat + codegen-units=1, отрисовка по требованию (без постоянного цикла), растеризация только видимых строк, дисковый кэш аватарок, ~5 HTTP-запросов на полный рефреш 200 друзей.

## Дорожная карта

1. **M1 (текущая):** вход + меню в стиле Roblox (складной сайдбар, полоса друзей, поиск и карточки плейсов, заявки в друзья) со статусами ✔
2. **M2:** подключение к реальным игровым серверам (официальный UDP-транспорт на базе RakNet) и рендеринг плейсов на wgpu — кнопка **Join** на карточках уже на месте
3. **M3:** чат, уведомления, страницы Charts / Avatar (неактивные пункты сайдбара)
4. **M4:** полный пайплайн рендеринга плейса

## Лицензия

**GPL-3.0-only** — см. [LICENSE](LICENSE). Шрифты DejaVu — свободная лицензия (`assets/fonts/LICENSE-DejaVu.txt`).
