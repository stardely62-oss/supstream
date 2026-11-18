# Supstream

Локальный инструмент для стримеров: донаты через DonationAlerts, награды за баллы канала, чат и события
Twitch в одной ленте, своё API для интерактивов. Всё работает на ПК стримера, без комиссий и без облака.

> Статус: **этап 1 из 6.** Работают ядро, подключения к Twitch и DonationAlerts, лента событий и API.
> Музыка с проверкой треков, система действий и алерты — следующие этапы (см. [план](#план)).

## Как это устроено

```
supstream-setup.exe
├─ Ядро на Rust (tokio + axum)       crates/core
│   ├─ Twitch: вход по коду устройства, EventSub по WebSocket
│   ├─ DonationAlerts: OAuth, Centrifugo по WebSocket
│   ├─ Шина событий → лента, виджеты, правила, внешние скрипты
│   └─ Локальный API: HTTP + WebSocket на 127.0.0.1:7272
├─ Панель на Svelte                  ui/        (вшивается в exe)
└─ Оболочка Tauri                     src-tauri/ (окно + трей, системный WebView2)
```

Окно приложения открывает ту же панель, что доступна в браузере, поэтому позже она без доработок откроется
и с телефона. Крестик прячет окно в трей, а ядро продолжает работать во время стрима.

## Первая настройка

### Twitch (один раз — автору сборки)

1. Включите двухфакторную аутентификацию на аккаунте Twitch.
2. Откройте [Twitch Developer Console](https://dev.twitch.tv/console/apps) → **Register Your Application**:
   - **OAuth Redirect URL:** `http://localhost`
   - **Category:** Application Integration
   - **Client Type: Public** — вход по коду устройства, секрет не нужен.
3. Скопируйте **Client ID** и либо:
   - вшейте его в сборку: `SUPSTREAM_TWITCH_CLIENT_ID=<id>` при `cargo build`, либо секрет репозитория
     `SUPSTREAM_TWITCH_CLIENT_ID` для GitHub Actions. Тогда друзьям ничего вводить не нужно;
   - либо вставьте его в панели: **Подключения → Twitch**.

Дальше каждый стример жмёт **«Войти через Twitch»** и подтверждает код на twitch.tv/activate.

### DonationAlerts (каждому стримеру)

1. [Приложения DonationAlerts](https://www.donationalerts.com/application/clients) → создать приложение.
2. URL перенаправления: `http://127.0.0.1:7272/auth/donationalerts/callback` (панель покажет его с кнопкой
   «Копировать»).
3. Вставьте ID и API-ключ в панели и нажмите **«Войти через DonationAlerts»**.

## Разработка

Нужны Rust (stable) и Node.js 22 + pnpm.

```bash
pnpm --dir ui install
```

```bash
pnpm --dir ui build
```

```bash
cargo run -p supstream-headless
```

Последняя команда запускает ядро без окна и печатает ссылку на панель. Для горячей перезагрузки интерфейса
держите ядро запущенным и в соседнем терминале запустите Vite (`/api` проксируется в ядро):

```bash
pnpm --dir ui dev
```

Десктоп-приложение целиком (на Linux нужны `libwebkit2gtk-4.1-dev` и `libayatana-appindicator3-dev`):

```bash
pnpm install && pnpm tauri dev
```

Установщик Windows собирается в GitHub Actions (`.github/workflows/build.yml`) — артефакт
`supstream-windows-setup`.

### Тесты

```bash
cargo test -p supstream-core
```

Сквозной тест `crates/core/tests/flows.rs` поднимает имитацию серверов Twitch и DonationAlerts и
проверяет всю цепочку: вход → токены → подписки → событие → WebSocket панели.

### Полезные переменные окружения

| Переменная | Зачем |
|---|---|
| `SUPSTREAM_DATA_DIR` | своя папка данных вместо `%APPDATA%` |
| `SUPSTREAM_NO_BROWSER=1` | не открывать браузер (сервер без экрана) |
| `SUPSTREAM_TWITCH_EVENTSUB_URL`, `SUPSTREAM_TWITCH_HELIX_URL` | mock-сервер из [Twitch CLI](https://dev.twitch.tv/docs/cli/) |
| `RUST_LOG=debug` | подробный лог |

## API

Ключ доступа — в `config.json` (поле `api_token`) и в панели: **Настройки → Ключ доступа**. Передаётся
заголовком `Authorization: Bearer <ключ>` или параметром `?token=<ключ>`.

| Метод | Путь | Что делает |
|---|---|---|
| GET | `/api/ws` | WebSocket: `hello`, затем поток `event` и `status` |
| GET | `/api/status` | версия и состояние подключений |
| GET | `/api/events?limit=100` | последние события |
| POST | `/api/events` | опубликовать своё событие `{ "type": "...", "data": {...} }` |
| POST | `/api/events/test` | тестовое событие `{ "type": "donation" }` |
| GET/PUT | `/api/settings` | настройки подключений |

Формат события:

```json
{
  "id": "…", "at": "2026-11-18T18:09:24Z", "source": "donationalerts", "test": false,
  "type": "donation",
  "data": { "donor": "Ivan", "amount": 500, "currency": "RUB", "message": "Привет!" }
}
```

Типы: `donation`, `redemption`, `chat_message`, `follow`, `subscription`, `gift_subs`, `cheer`, `raid`.

## План

1. ✅ Ядро, шина событий, Twitch и DonationAlerts, лента, API.
2. Система «событие → условие → действие», интеграция с OBS (obs-websocket), награды за баллы, созданные
   приложением (с возвратом баллов).
3. Музыка: очередь заказов, плеер-виджет на отдельной дорожке, проверка текстов (готовый текст + выравнивание
   по звуку, распознавание, фильтр, глушение слов, минусовка), тревожная кнопка.
4. Алерты и виджеты с шаблонами.
5. Озвучка (локальная), чат-бот, плагины.
6. Визуальные редакторы, панель с телефона, Stream Deck.
