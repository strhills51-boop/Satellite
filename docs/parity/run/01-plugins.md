# Паритет пробега: плагины (монолит CP9 + v611)

Источник: `docs/semantic/monolith/05-run.md` §4.6 (CP9), S11 §2 (`parsePluginHeaderFromFile` 100666), `resolvePluginType`
100529 / `resolvePluginTypeV503` 172032.

| Монолит | NG | Статус |
|---|---|---|
| Только корневые `.esp/.esm/.esl` loose (не из архивов) | анализатор `plugins`: `origin loose ∧ depth 0 ∧ ext ∈ esp/esm/esl` | совпадает |
| TES4: ESM `0x1`, ESL `0x200`, `MAST/CNAM/SNAM/HEDR`, `XXXX` | `sat-formats::plugin::read_plugin_header` | совпадает (проверено на реальном ESL сборки владельца: HEDR 140 = 103 записи + 37 групп) |
| Скан записей v611: 24-байтные заголовки, стек GRUP, `TES4` не считается, ссылки = REFR + ACHR | `scan_records` → `RecordCounts` (`--records`, настройка штампа анализатора) | совпадает по счёту; статусы `partial/unavailable` монолита = `error` в `PluginFacts` |
| Тип `resolvePluginType` (`espfe`, `unknown`) | `monolith_plugin_type` — дословно, для паритетных видов и импорта | совпадает |
| ESM-флаг на `.esp` → `esp` (расширение раньше флага) | `PluginHeader::kind` → `Esm` | **сознательное расхождение**: игра грузит такой плагин вместе с мастерами; для подсчёта лимитов и порядка важен класс загрузки |
| Сбой чтения → запись `recordScanStatus 'error'`, пробег продолжается | политика `optional`: ошибка в `PluginFacts.error` и в `issues` пробега, остальное у мода цело | совпадает |
| Дедуп по нормализованному пути (первый выигрывает) | корневые имена уникальны в папке (NTFS без учёта регистра) | не нужен |

Открыто: метаданные победителя для логических групп (v545) — Фаза 3.
