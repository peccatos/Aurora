# plan.md — слабые и незаконченные места проекта

Локальный трекер (git-ignored). Статусы: 🔴 блокирует релиз, 🟠 нужно до
продакшена, 🟡 желательно, 🟢 мелочь. Обновляется по мере закрытия.
**Фича 5 — импорт плейлистов**: доступна с вашим аккаунтом (новая сессия Required после деплоя). В любой момент сделайте один `/api/oauth/youtube-music/begin`, пройдите Google авторизацию → аккаунт попадёт в настройки. Также можно добавить Spotify (unconfigured до сих пор).

Дата последнего аудита: 2026-11-09 (третий проход: Ф7 деплой live, OAuth live-верифицирован; выявлен регресс 401 навсегда на begin — исправлен и в проде c600295/9e0d15e; Google client secret сменён, encryption key сгенерирован заново).

---

## Фича 7 — HTTPS деплой, продакшен-конфиг (✅ ЗАКРЫТО 2026-11-09)

- ✅ **Деплой live на Render** (`aurora-api-3.onrender.com`): `Dockerfile`
  (release-бинарь, unprivileged, /data для SQLite) + `render.yaml`
  (docker runtime, /health, диск 1GB, секреты через sync:false);
  `main.rs` слушает `0.0.0.0:$PORT`. Оживлённый smoke: PORT, health,
  ready, ping, fail-closed status. Все env-vars в Render:
  `AURORA_API_ALLOWED_ORIGINS`, `AURORA_API_FRONTEND_URL=/#/settings`,
  `AURORA_OAUTH_ENCRYPTION_KEY` (new, base64url 32b),
  `AURORA_YOUTUBE_MUSIC_CLIENT_ID/SECRET/REDIRECT_URI`,
  `AURORA_DATABASE_PATH=/data/aurora.db`.
- ✅ **OAuth flow end-to-end верифицирован на проде** (2026-11-09):
  `POST /api/oauth/youtube-music/begin` → 200 + 3 куки
  (`__Host-aurora_session` 8h, `__Host-aurora_user` 180d, `__Host-aurora_csrf`);
  authorization URL на `accounts.google.com` с `access_type=offline&prompt=consent`;
  callback сохраняет грант под **user_hash** (миграция 0006); статус/плейлисты/дисконнект
  ищут грант по user_hash — сессия больше не отвязывает токены при истечении/ротации.
- **Ключ шифрования (`AURORA_OAUTH_ENCRYPTION_KEY`) — сгенерирован заново**
  (локальная копия `.aurora_encryption_key_local.txt`, в gitignore).
  Старые гранты нечитаемы — **пользователь должен переподключить аккаунт один раз**
  после деплоя миграции 0006 (ожидаемо, записано в логах деплоя).
- **Google client secret сменён** (`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`),
  старый `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` отозван в Google Cloud Console.
- **Rate limiter — in-process** (документировано, масштабирование — общий стор).
- **Consent screen в Production** — refresh-токены не истекают через 7 дней.
- **Rate limiter — in-process.** Задокументировано, но при масштабировании на
  2+ процесса у каждого свои счётчики. Для одного инстанса ок; при росте —
  общий стор (Redis и т.п.).
- **Клиентские секреты Spotify/Google** лежат в env/конфиге процесса.
  Секрет-менеджера нет. Приемлемо для одного сервера, неприемлемо для команды.
- **Сессионные cookies** — `__Host-` префикс требует HTTPS и одного хоста.
  Это же означает: ни проксирования с другим доменом, ни HTTP-дев-сервера
  без `localhost`-обхода. Не баг, но ограничение, о котором легко забыть.

## Фича 8 — yt-dlp (🟠 функционально готов, границы узкие)

- **Браузерный режим умеет только метаданные и только через
  `VITE_MEDIA_API_BASE_URL`.** Скачивание в браузере — Tauri-only by design
  (файлы пишутся на диск клиента). Если браузерному режиму нужен файл —
  сервер должен стримить его; этого нет и не запланировано в v0.2.
- ~~**`/api/media/*` на бэкенде не реализован.**~~ ✅ ЗАКРЫТО (2026-11-08):
  `crates/aurora-api/src/media.rs` — `POST /api/media/metadata`
  (session+CSRF, media rate bucket, sanitize DTO) и `GET /api/media/ping`;
  6 интеграционных тестов; фронтенд-фикс CSRF-заголовка + 9 юнит-тестов
  `media-download.test.ts`; ADR 0006 + PROVIDERS.md задокументировали.
- **Фронтенд не вызывает `/api/media/ping`** — ручка есть, но UI не прячет
  фичу, когда yt-dlp недоступен на сервере. Мелочь, UX-полировка.
- **Плейлист-эндпоинт апи не отдает YouTube `itemCount`** — поле вычищено как
  dead code после рефакторинга нормализации; фронтенд считает по трекам.
  Разница чисто косметическая (RemotePlaylist — общий domain-тип, добавление
  поля — ripple на все провайдеры), сознательно отложено.
- **Allowlist жёсткий и узкий** (archive.org, youtube.com, youtu.be). Это
  правильно по замыслу, но пользователи будут просить SoundCloud/Bandcamp —
  каждое добавление должно проходить через «это легальный источник?».
- **yt-dlp не версионируется пином.** Бинарники лежат в `bin/`, CI копирует
  как есть; нет проверки версии/хеша на CI. Апгрейд yt-dlp молча меняет
  поведение CLI-флагов (`--print after_move:filepath` уже менялся между
  версиями).
- **Linux/macOS пути yt-dlp не тестировались** — написаны по аналогии с
  Windows-путём (read_dir-поиск с triple-суффиксом). CI staging есть, ручного
  прогона нет.

## Фича 5 — импорт плейлистов (✅ ЗАКРЫТО 2026-11-09, проверено вживую)

- ✅ **Live end-to-end пройден с реальным аккаунтом Google**: begin → авторизация →
  Connected в UI → импорт плейлиста (7 треков) — работает. По пути исправлены
  три продовых бага:
  1. `AURORA_API_FRONTEND_URL` без хэш-роутера → «invalid frontend URL» на старте
     (фикс `5b8b3b7`: `/#/settings`).
  2. Браузер не отправляет `Origin` на same-origin fetch → 403 на status/playlists
     (фикс `b93b154`: fallback на `Referer` в `origin_ok`; фронт также шлёт Origin явно `92c39a5`).
  3. `deny_unknown_fields` в YouTube-парсере отвергал конверт-поля реального
     ответа playlistItems (`kind`/`etag`/`id`/`pageInfo`) → 502 invalid response
     (фикс `06d2156`: терпимый парсинг + тест на реальный формат).
- **UX-долг:** флоу импорта при 401 не перевыпускает сессию сам — пользователь
  обязан зайти в Settings → Connect. Записано в Roadmap (блок UX).
- ~~**`frontend/src/remote-playlist-import.ts`** — DTO-скаффолд на 38
  строк.~~ ✅ НЕАКТУАЛЬНО (2026-11-08): модуль вырос в полноценный слой
  (395 строк: match-kind классификация, buildLocalTrackIndex,
  matchRemoteTracks, applyRemotePlaylistImport, remoteImportFlow), 5
  импортёров, собственные тест-сьюты (remote-playlist-import.test.ts,
  remote-playlist-matching.test.ts). «Слить» не с чем — это и есть
  единая точку логики импорта.

## Фича 6 — безопасность (🟡 фундамент solid, периферия сырая)

- ✅ **Критический баг «401 навсегда» на begin исправлен и в продакшене** (2026-11-09,
  коммиты `c600295` + `9e0d15e`). Проблема: неизвестная/просроченная
  session-cookie (после миграции 0006 или 8h expiry) делала `begin` → 401
  навсегда, блокируя любое подключение до ручной очистки cookie. Исправление:
  `begin` — это bootstrap сессии; при неизвестном, но well-formed
  серверным значении cookie теперь перевыпускается новая сессия (Set-Cookie),
  а 180-дневная `__Host-aurora_user` не трогается — гранты остаются
  достижимыми. Тесты обновлены под новое поведение.
- **CSRF-проверка требует точного совпадения Origin с allowlist.** Любой новый
  домен фронтенда = код-изменение в `aurora-api`. Для v0.2 ок; для
  мультидоменного деплоя — конфиг.
- **Нет security-заголовков на статике** (CSP на фронтенде отдает хостинг;
  server-side CSP для /api нет — Content-Type: application/json + nosniff
  есть, остальное не настроено).
- **Логи не скраббятся от URL'ов провайдеров** — в authorization URL есть
  state-токен; в трейсинг может утекать. Утечка не критична (state одноразовый
  и хешируется в БД), но вычищать надо.
- ~~**`#![allow(dead_code)] csrf_matches_session`**~~ ✅ ЗАКРЫТО
  (2026-11-08): dead-code reference-проверка удалена; авторитетная
  storage-bound проверка `authorize_csrf` покрыта интеграционными тестами.

## Фича 3 — refresh/rotation (🟡)

- ~~**Concurrent refresh не сериализуется.**~~ ✅ ЗАКРЫТО (2026-11-08):
  `token::RefreshGate` — per-account mutex + double-check по consumed
  refresh-токену; второй конкурентный вызов получает кешированную
  ротацию, а не `invalid_grant`. Тест
  `concurrent_expired_requests_refresh_exactly_once`.
- **Expire-политика access-токенов — «рефреши при 401».** Нет проактивного
  рефреша по expires_in (когда провайдер его даёт). Работает, но каждая
  сессия начинается с одного 401-round-trip'а. (RefreshGate уже делает
  этот round-trip дешёвым и безопасным — осознанно отложено.)

## Фича 2 — хранилище (🟢 почти чисто)

- **SQLite VACUUM/retention для `oauth_transactions`** — одноразовые
  state-строки чистятся лениво (cleanup на запросе, уже на begin);
  фонового периодического cleanup нет. Rate limiter ограничивает скорость
  роста, одноразовый claim — ценность старых строк. Осознанно оставлено:
  фоновая задача в API — новый класс отказов (shutdown, блокировки
  max_connections(1) у пула), выгода — гигиены таблицы в one-instance
  деплое. Вернуться, если появятся multi-instance деплои.
- ~~**`ignored`-тест `rejects_foreign_origin`**~~ ✅ ЗАКРЫТО (2026-11-08):
  ignore снят, тест зелёный (state на файловой БД, путь без cookies не
  ходит в хранилище). Игноров в сьюте больше нет: 47 lib + 40 integration.

## Инфраструктура / процесс (🟡)

- **CI не гоняет integration-тесты `aurora-api/tests/` отдельно** — они
  попадают в `cargo test --workspace`, но бандл-джобы Windows-агента
  нестабильны по времени; нет отдельного fast-lane.
- **`frontend/aurora_design_check_tmp.py` и
  `frontend/.tmp-research-spotify2ytm/`** — мусор в рабочем дереве (не в
  git), убрать локально.
- **`bin/` не в git** (правильно — бинарники не коммитим), но release.yml
  **упадёт, если bin/yt-dlp отсутствует** на теге. Это by design (hard-fail),
  значит деплой-машине нужен checkout bin/ вне git. Документировано в
  release.yml, но легко забыть.
- **Документация ADR для OAuth-архитектуры** — `docs/ADR/0006-oauth-backend.md`
  (BFF + yt-dlp) и `docs/PROVIDERS.md` есть. `docs/ARCHITECTURE.md` не
  упоминает aurora-api/aurora-providers — обновить при следующем проходе.

## Что точно закрыто (для контраста)

- Ф1 (server-side OAuth exchange + status): ✅ код + тесты.
- Ф2 (encrypted storage): ✅ AES-256-GCM, миграция 0005, тесты.
- Ф3 (refresh/rotation/reauth): ✅ с тестами на transient vs definitive.
- Ф4 (server-side playlist API): ✅ list + detail, пагинация, caps.
- Ф6 (CSRF/rate-limit/tests): ✅ 42+34 теста зелёные, clippy -D warnings чист.
- Ф7 (HTTPS деплой, продакшен-конфиг): ✅ live на Render (aurora-api-3.onrender.com),
  OAuth end-to-end верифицирован, секреты сменены, миграция 0006 применена.
- Ф8 Tauri-путь: ✅ метаданные + скачивание с allowlist.
