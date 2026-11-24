# rentlyClaudeNote — «Claude ажил» гүйцэтгэгч (Rust, Windows Service)

`autologRust`-ийн бүтэцтэй ижил: `rentlyClaudeNote.exe`-ийн хажууд `config.toml`, `rentlyClaudeNote.log`, `data/` байна.

Ялгаа нь өгөгдлөө serverKhyanalt-аас биш, turees-ийн **«Claude ажил»** цонхны (`/khyanalt/claudeAjil`, зөвхөн Zevtabs `CAdmin1`) ажлын жагсаалтаас авна. Цонхонд ажлын тайлбар бичиж, зураг (Ctrl+V screenshot) хавсаргаж, ажиллах repo-г сонгоод **«Claude-д илгээх»** дарна. Service `shalgakh_secs` тутам шалгаж, хүлээгдэж буй ажлыг хуучнаас нь эхэлж нэг нэгээр нь `claude -p`-ээр гүйцэтгэнэ.

## Ажлын төлөв

| Төлөв | Утга |
|---|---|
| `noorog` | Бичиж байгаа — service авахгүй |
| `shine` | «Claude-д илгээх» дарсан, хүлээгдэж байна |
| `khiigdej` | Service авсан, Claude ажиллаж байна (цонхонд явц 10 сек тутам шинэчлэгдэнэ) |
| `duussan` / `aldaa` | Үр дүн, өөрчлөгдсөн файлууд, зардал цонхонд харагдана. «Дахин ажиллуулах» → `shine` |
| `tsutsalsan` | Цонхноос цуцалсан — service ажиллаж байсан бол Claude-г зогсооно |

## Endpoint-ууд (`tureesBack routes/claudeAjilRoute.js`)

| Endpoint | Хэрэглээ |
|---|---|
| `POST /ajiltanNevtrey` | JWT (config-д `token` байхгүй эсвэл 401 болсон үед) |
| `GET /claudeAjil?tuluv=shine` | Хүлээгдэж буй ажлууд (хуучнаас нь) |
| `POST /claudeAjil/:id/avya` | `shine → khiigdej` atomic түгжилт. 409 бол өөр service авсан → алгасна |
| `GET /zuragAvya/claudeAjil/<baiguullagiinId>/<id>` | Хавсаргасан зураг → `data/<ajilId>/zurag-N.png` |
| `POST /claudeAjil/:id/yavts` | 15 сек тутам явцын мөрүүд; хариуны `tuluv = tsutsalsan` бол Claude-г зогсооно |
| `POST /claudeAjil/:id/ur` | `duussan`/`aldaa`, Claude-ийн тайлан, `git status` мөрүүд, зардал |

Бүгд `tokenShalgakh` + `adminShalgay` (Zevtabs `CAdmin1`)-аар хамгаалагдсан (`zuragAvya`-аас бусад).

## Repo

Цонхонд `tureesBack`, `turees`, `parking-v2`, `quickqpaypackv2` сонгоно (`models/claudeAjil.js` `REPOS`). Нэр бүрийн local замыг `config.toml`-ийн `[repos]`-д тохируулна. Эхний repo нь Claude-ийн ажлын хавтас, бусад нь `--add-dir`.

`bokhir_repo_zuvshuurukh = false` (анхдагч) үед сонгосон repo-д commit хийгдээгүй өөрчлөлт байвал Claude ажиллуулахгүй, ажил `aldaa` болно — commit хийгээд «Дахин ажиллуулах».

`git_pull = true` (анхдагч) үед ажил эхлэхийн өмнө **service өөрөө** repo бүрт `git pull --ff-only` хийнэ — Claude хамгийн сүүлийн код дээр ажиллаж, дараа нь таны `git pull` merge зөрчилтэй болохоос сэргийлнэ. Цэвэр repo-г шинэчилж чадаагүй бол (сүлжээ, салаалсан local commit) ажил `aldaa` болно. Өмнөх ээлжийн commit хийгдээгүй өөрчлөлттэй repo-д pull зөрчилтэй бол git юу ч өөрчлөхгүй тул ⚠ анхааруулаад үргэлжлүүлнэ. Upstream-гүй branch-ийг алгасна. Claude-д `git pull` эрх өгөхгүй (ажлын дунд код солигдохгүй).

`[claude] avtomat_push`-д нэрлэсэн repo-д (жишээ нь `["irtsBack", "irtsWeb"]`) ажил **амжилттай** дуусахад service өөрөө Claude-ийн өөрчилсөн файлуудыг л `git add` → `git commit` (гарчиг = ажлын гарчиг) → `git push` хийнэ (`src/push.rs`). Хэрэглэгчийн өөрчлөлт, `.env*` commit-д орохгүй; урьдчилан stage хийсэн зүйл байвал commit хийхгүй. Force push хийхгүй — remote урагшилсан бол нэг удаа `pull --rebase --autostash`, зөрчилтэй бол rebase-ийг цуцалж commit local-д үлдэнэ. Үр дүн цонхны явц ба тайланд `⬆`/`⚠` мөрөөр харагдана. Бусад repo-д commit/push-ийг хүн хийсээр.

`--add-dir` repo-уудын `CLAUDE.md` ч ачаалагдана (`CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1`).

## Claude-ийн хамгаалалт

Read/Glob/Grep/Edit/Write, `git status/diff/log/show` (`git -C <repo>` хэлбэрээр ч), `node --test/--check`, `npm test`, `npm run lint/build`, `npx tsc/eslint` зөвшөөрөгдсөн. Хориотой: `git commit/push/pull/reset/checkout/switch/restore/clean/stash/rebase/merge/revert/cherry-pick/am/apply/config`, `npm install/publish`, `pm2`, WebFetch/WebSearch. `--max-budget-usd`, `timeout_min`. Өөрчлөлт зөвхөн working tree-д үлдэнэ — өөрөө хянаж commit эсвэл буцаана.

## Дизайныг зурагтай харьцуулах

Ажил бүрт `data/<ajilId>/shalgalt/` хавтас үүсэж, дотор нь `zuragBolgokh.mjs` туслах бичигдэнэ. Claude зөвхөн энэ хавтсан дахь script-ээ `node`-оор ажиллуулж, үр дүнг Edge/Chrome-оор PNG болгоод хавсаргасан загвар зурагтай таартал харьцуулна. Анхдагч загвар нь Opus 5.5 — `claude-opus-5-5` (`[claude] model`).

`data/<ajilId>/` дотор `prompt.md`, татсан зургууд, `shalgalt/`, `claude.jsonl` (бүх stream event — аудит) хадгалагдана.

## Ашиглах

```powershell
cargo build --release
copy config.example.toml target\release\config.toml   # nuutsUg эсвэл token, [repos] бөглөнө

target\release\rentlyClaudeNote.exe once   # нэг удаа шалгаж хүлээгдэж буй ажлыг гүйцэтгэнэ
target\release\rentlyClaudeNote.exe run    # console горим

# Administrator PowerShell:
target\release\rentlyClaudeNote.exe install
sc config RentlyClaudeNote obj= ".\pc" password= "<Windows нууц үг>"
sc start RentlyClaudeNote
sc stop  RentlyClaudeNote
target\release\rentlyClaudeNote.exe uninstall
```

**Service ба Claude-ийн нэвтрэлт:** LocalSystem account-д Claude-ийн нэвтрэлт байхгүй тул service-ийг Claude нэвтэрсэн Windows хэрэглэгчээр ажиллуулна (дээрх `sc config`), эсвэл Task Scheduler-ээр нэвтрэх үед `rentlyClaudeNote.exe run`.

Service зогсоход ажиллаж байсан Claude зогсож ажил `khiigdej`-д үлдэнэ; дараагийн эхлэлд тухайн компьютерийн (`COMPUTERNAME`) гацсан ажлуудыг `aldaa` болгоно.
