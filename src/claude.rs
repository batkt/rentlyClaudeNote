// «Claude ажил»-ын тайлбар, зургаар Claude Code-д (headless `claude -p`) даалгавар өгнө.
//
// Хамгаалалт:
//  - Commit хийгдээгүй өөрчлөлттэй repo-д ажиллахгүй (bokhir_repo_zuvshuurukh = false үед).
//  - Зөвхөн ZUVSHUURUL дахь tool-ууд; -p горимд бусад нь автоматаар татгалзагдана.
//  - git commit/push/reset/checkout, npm install, pm2, вэб хандалтыг тусгайлан хориглоно.
//  - --max-budget-usd ба timeout.

use serde::Deserialize;
use serde_json::Value;
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    process::Command,
};

const ZUVSHUURUL: &[&str] = &[
    "Read",
    "Skill",
    "Glob",
    "Grep",
    "Edit",
    "Write",
    "Bash(git status:*)",
    "Bash(git diff:*)",
    "Bash(git log:*)",
    "Bash(git show:*)",
    "Bash(node --test:*)",
    "Bash(node --check:*)",
    "Bash(npm test:*)",
    "Bash(npm run lint:*)",
    "Bash(npm run build:*)",
    "Bash(npx tsc:*)",
    "Bash(npx eslint:*)",
];

const KHORIG: &[&str] = &[
    "Bash(npm install:*)",
    "Bash(npm publish:*)",
    "Bash(pm2:*)",
    "WebFetch",
    "WebSearch",
];

/// Бусад repo-г `git -C <зам> ...`-ээр зөвхөн уншина (cwd нь эхний repo).
const GIT_UNSHIKH: &[&str] = &["status", "diff", "log", "show"];

/// `git <команд>` ба `git -C <зам> <команд>` хоёуланд нь хориотой.
const GIT_KHORIG: &[&str] = &[
    "commit", "push", "pull", "reset", "checkout", "switch", "restore", "clean", "stash", "rebase",
    "merge", "revert", "cherry-pick", "am", "apply", "config",
];

/// Ажлын shalgalt/ хавтсанд Claude-ийн бичсэн туршилтын script-ийг л ажиллуулах эрх.
fn zuvshuurul_jagsaalt(shalgalt_dir: &str) -> Vec<String> {
    let mut v: Vec<String> = ZUVSHUURUL.iter().map(|s| s.to_string()).collect();
    for k in GIT_UNSHIKH {
        v.push(format!("Bash(git -C * {k})"));
        v.push(format!("Bash(git -C * {k} *)"));
    }
    v.push(format!("Bash(node {shalgalt_dir}/*)"));
    v.extend(skill_script_erkh(std::env::var("USERPROFILE").ok().as_deref()));
    v
}

/// ~/.claude/skills-д суулгасан skill-ийн туслах script-үүд. Зөвхөн тухайн
/// script-ийн үнэмлэхүй замаар ажиллуулах эрх (дурын `python <файл>` биш).
const SKILL_SCRIPTUUD: &[&str] = &["ui-ux-pro-max/scripts/search.py"];

fn skill_script_erkh(home: Option<&str>) -> Vec<String> {
    let Some(home) = home.map(|h| h.trim().trim_end_matches(['\\', '/'])).filter(|h| !h.is_empty())
    else {
        return Vec::new();
    };
    let home = home.replace('\\', "/");
    let mut v = Vec::new();
    for s in SKILL_SCRIPTUUD {
        for py in ["python", "python3"] {
            v.push(format!("Bash({py} {home}/.claude/skills/{s})"));
            v.push(format!("Bash({py} {home}/.claude/skills/{s} *)"));
        }
    }
    v
}

fn khorig_jagsaalt() -> Vec<String> {
    let mut v: Vec<String> = KHORIG.iter().map(|s| s.to_string()).collect();
    for k in GIT_KHORIG {
        v.push(format!("Bash(git {k}:*)"));
        v.push(format!("Bash(git -C * {k})"));
        v.push(format!("Bash(git -C * {k} *)"));
    }
    v
}

const ZURAG_BOLGOKH_JS: &str = include_str!("zuragBolgokh.mjs");

/// Ажлын туршилтын хавтсын үнэмлэхүй зам (`/` тусгаарлагчтай — эрхийн дүрэм, prompt-д ижил).
fn shalgalt_zam(ajil_dir: &Path) -> Result<String, String> {
    let zam = std::path::absolute(ajil_dir.join("shalgalt"))
        .map_err(|e| format!("{}: {e}", ajil_dir.display()))?;
    Ok(zam.display().to_string().replace('\\', "/"))
}

/// shalgalt/ хавтас, зураг гаргах туслахыг бэлдэж хавтсын замыг буцаана.
pub fn shalgalt_beltgey(ajil_dir: &Path) -> Result<String, String> {
    let zam = shalgalt_zam(ajil_dir)?;
    std::fs::create_dir_all(&zam).map_err(|e| format!("{zam}: {e}"))?;
    std::fs::write(format!("{zam}/zuragBolgokh.mjs"), ZURAG_BOLGOKH_JS)
        .map_err(|e| format!("{zam}/zuragBolgokh.mjs: {e}"))?;
    Ok(zam)
}

#[derive(Deserialize, Clone)]
pub struct ClaudeTokhirgoo {
    #[serde(default = "default_exe")]
    pub exe: String,
    #[serde(default = "default_budget")]
    pub budget_usd: f64,
    #[serde(default = "default_timeout")]
    pub timeout_min: u64,
    /// Хоосон бол Claude Code-ийн анхдагч загвар (headless горимд Sonnet)
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default)]
    pub bokhir_repo_zuvshuurukh: bool,
    /// Ажил эхлэхийн өмнө repo бүрийг `git pull --ff-only`-оор шинэчилнэ —
    /// Claude хуучирсан код дээр засаад дараа нь merge зөрчил үүсгэхээс сэргийлнэ.
    #[serde(default = "default_true")]
    pub git_pull: bool,
    /// Ажил амжилттай дуусахад service өөрөө Claude-ийн өөрчилсөн файлыг commit +
    /// push хийх repo-ууд (жишээ нь ["irtsBack", "irtsWeb"]). Хоосон бол аль нь ч биш.
    #[serde(default)]
    pub avtomat_push: Vec<String>,
}

impl Default for ClaudeTokhirgoo {
    fn default() -> Self {
        Self {
            exe: default_exe(),
            budget_usd: default_budget(),
            timeout_min: default_timeout(),
            model: default_model(),
            bokhir_repo_zuvshuurukh: false,
            git_pull: true,
            avtomat_push: Vec::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_exe() -> String {
    "claude".into()
}
fn default_model() -> String {
    "claude-opus-5-5".into()
}
fn default_budget() -> f64 {
    5.0
}
fn default_timeout() -> u64 {
    60
}

/// Prompt-д оруулах ажлын агуулга
pub struct Ajil<'a> {
    pub garchig: &'a str,
    pub tailbar: &'a str,
    /// Цонхонд сонгосон систем (tureesBack models/claudeAjil.js#SYSTEMUUD-ийн utga)
    pub system: &'a str,
    /// (repo нэр, local зам)
    pub repos: &'a [(String, String)],
    /// Системд хамаарах боловч энэ компьютер дээр олдоогүй repo-ууд
    pub baikhgui: &'a [String],
    /// (local файлын зам, зургийн тайлбар)
    pub zurguud: &'a [(String, String)],
    /// Энэ ээлжийн нэмэлт заавар — хоосон бол анхны ээлж
    pub nemelt_zaavar: &'a str,
    /// Өмнөх ээлжүүд, хуучнаас нь (backend-ийн `tuukh`)
    pub tuukh: &'a [UmnukhEelj],
    /// Өмнөх ээлжийн Claude-ийн commit хийгдээгүй өөрчлөлттэй repo-ууд
    pub umnukh_uurchlult: &'a [String],
    /// Туршилтын script, зураг гаргах хавтас (`shalgalt_beltgey`)
    pub shalgalt_dir: &'a str,
    /// Хэрэглэгчийн шийдэх зүйл гарвал Claude зогсоод асууж болох эсэх
    /// (ажлын `asuultAsuukh`, анхдагч true)
    pub asuukh: bool,
    /// Асуулт тавьж зогссон өмнөх удаагийн Claude-ийн тэмдэглэл (хоосон бол асуугаагүй)
    pub oilgolt: &'a str,
    /// Хэрэглэгчийн хариулсан (эсвэл алгассан) тодруулах асуултууд
    pub asuultuud: &'a [Asuult],
}

// ─── Ажлын дундах тодруулах асуулт ───────────────────────────────────────────
//
// Claude ажлыг шууд гүйцэтгэнэ. Зөвхөн хэрэглэгчийн шийдэх ёстой зүйлтэй
// тулгарвал зогсож `asuultuud`-д асуумж буцаана; хэрэглэгч хариулахад ажил
// хариулттайгаа дахин ажиллана.

/// Асуултын нэг сонголт (backend models/claudeAjil.js#songoltSchema)
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, Deserialize)]
pub struct Songolt {
    pub ner: String,
    #[serde(default)]
    pub tailbar: String,
    /// Claude-ийн санал болгож буй сонголт
    #[serde(default)]
    pub sanal: bool,
}

/// Claude-ийн тодруулах асуулт ба хэрэглэгчийн хариулт (backend asuultSchema)
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, Deserialize)]
pub struct Asuult {
    pub asuult: String,
    /// Богино шошго (жишээ нь «Хуваарилалт»)
    #[serde(default)]
    pub tovch: String,
    #[serde(default, rename = "olonSongolt")]
    pub olon_songolt: bool,
    #[serde(default)]
    pub songoltuud: Vec<Songolt>,
    /// Хэрэглэгчийн сонгосон сонголтуудын `ner`
    #[serde(default)]
    pub khariu: Vec<String>,
    /// Хэрэглэгчийн чөлөөт хариулт («Бусад»)
    #[serde(default)]
    pub busad: String,
    /// Хэрэглэгч хариулсан эсвэл алгассан
    #[serde(default)]
    pub khariulsan: bool,
}

/// Claude-ийн эцсийн бүтэцтэй хариу (`structured_output`)
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AjliinUr {
    /// Markdown тайлан. Асуулттай бол — юу олсон, юу хийсэн, яагаад асууж байгаа
    pub tailan: String,
    /// Хоосон бол ажил дууссан
    pub asuultuud: Vec<Asuult>,
}

pub const ASUULT_MAX: usize = 4;
const SONGOLT_MIN: usize = 2;
const SONGOLT_MAX: usize = 4;
const TAILAN_MAX: usize = 100_000;

/// `--json-schema` — `result` event-ийн `structured_output` энэ бүтэцтэй ирнэ.
/// `asuukh = false` бол асуулт огт буцааж чадахгүй (maxItems 0).
pub fn ajil_schema(asuukh: bool) -> Value {
    let songolt = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["ner", "tailbar", "sanal"],
        "properties": {
            "ner": { "type": "string", "description": "Сонголтын товч нэр (1-6 үг)" },
            "tailbar": { "type": "string", "description": "Энэ сонголтыг хийвэл юу болох, давуу/сул тал" },
            "sanal": { "type": "boolean", "description": "Чиний санал болгож буй сонголт эсэх" }
        }
    });
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["tailan", "asuultuud"],
        "properties": {
            "tailan": {
                "type": "string",
                "description": "Markdown тайлан (цонхонд харагдана). Асуулттай бол: юу олсон, юу хийсэн, яагаад зогссон"
            },
            "asuultuud": {
                "type": "array",
                "description": "Зөвхөн хэрэглэгчийн шийдэх ёстой зүйлтэй тулгарсан бол. Ажил дууссан бол хоосон",
                "maxItems": if asuukh { ASUULT_MAX } else { 0 },
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["asuult", "tovch", "olonSongolt", "songoltuud"],
                    "properties": {
                        "asuult": { "type": "string", "description": "Бүрэн асуулт, асуултын тэмдгээр төгсөнө" },
                        "tovch": { "type": "string", "description": "12 тэмдэгтээс богино шошго" },
                        "olonSongolt": { "type": "boolean", "description": "Олон сонголт зэрэг сонгож болох эсэх" },
                        "songoltuud": {
                            "type": "array",
                            "minItems": SONGOLT_MIN,
                            "maxItems": SONGOLT_MAX,
                            "items": songolt
                        }
                    }
                }
            }
        }
    })
}

fn tsever(s: &str, n: usize) -> String {
    s.trim().chars().take(n).collect()
}

/// `structured_output`-ийг шалгаж цэвэрлэнэ. Schema-тай ч Claude/CLI-ийн
/// алдаанаас хамгаалж хоосон асуулт, сонголтгүй асуулт, давхардсан сонголтыг
/// хаяж, тоо, уртыг хязгаарлана.
pub fn ajliin_ur_tseverley(v: &Value) -> Result<AjliinUr, String> {
    #[derive(Deserialize)]
    struct Tuukhii {
        #[serde(default)]
        tailan: String,
        #[serde(default)]
        asuultuud: Vec<Asuult>,
    }
    let t: Tuukhii = serde_json::from_value(v.clone())
        .map_err(|e| format!("Claude-ийн JSON хариу буруу бүтэцтэй: {e}"))?;
    let asuultuud = t
        .asuultuud
        .into_iter()
        .filter_map(|a| {
            let mut songoltuud: Vec<Songolt> = Vec::new();
            for s in a.songoltuud {
                let ner = tsever(&s.ner, 100);
                if ner.is_empty() || songoltuud.iter().any(|x| x.ner == ner) {
                    continue;
                }
                songoltuud.push(Songolt {
                    ner,
                    tailbar: tsever(&s.tailbar, 500),
                    sanal: s.sanal,
                });
            }
            songoltuud.truncate(SONGOLT_MAX);
            let asuult = tsever(&a.asuult, 500);
            (!asuult.is_empty() && songoltuud.len() >= SONGOLT_MIN).then(|| Asuult {
                asuult,
                tovch: tsever(&a.tovch, 30),
                olon_songolt: a.olon_songolt,
                songoltuud,
                khariu: Vec::new(),
                busad: String::new(),
                khariulsan: false,
            })
        })
        .take(ASUULT_MAX)
        .collect();
    Ok(AjliinUr {
        tailan: tsever(&t.tailan, TAILAN_MAX),
        asuultuud,
    })
}

/// Ажлын prompt-д оруулах тодруулгын хэсэг
fn todruulga_khesg(a: &Ajil) -> String {
    if a.oilgolt.trim().is_empty() && a.asuultuud.is_empty() {
        return String::new();
    }
    let oilgolt = if a.oilgolt.trim().is_empty() {
        String::new()
    } else {
        format!(
            "Өмнө нь асуулт тавьж зогсохдоо бичсэн чиний тэмдэглэл:\n{}\n\n",
            taslakh(a.oilgolt, SUULIIN_TAILAN_MAX)
        )
    };
    let asuultuud = a
        .asuultuud
        .iter()
        .enumerate()
        .map(|(i, q)| {
            let songoson: Vec<String> = q
                .khariu
                .iter()
                .map(|k| match q.songoltuud.iter().find(|s| &s.ner == k) {
                    Some(s) if !s.tailbar.trim().is_empty() => format!("{k} ({})", s.tailbar.trim()),
                    _ => k.clone(),
                })
                .collect();
            let mut khariu = songoson.join("; ");
            if !q.busad.trim().is_empty() {
                if !khariu.is_empty() {
                    khariu.push_str("\n   Нэмэлт тайлбар: ");
                }
                khariu.push_str(q.busad.trim());
            }
            if khariu.is_empty() {
                let sanal: Vec<&str> = q
                    .songoltuud
                    .iter()
                    .filter(|s| s.sanal)
                    .map(|s| s.ner.as_str())
                    .collect();
                khariu = if sanal.is_empty() {
                    "(хариулаагүй — хамгийн боломжит хувилбарыг сонгоод тайланд тэмдэглэ)".into()
                } else {
                    format!("(хариулаагүй — санал болгосон «{}»-г ашигла)", sanal.join("», «"))
                };
            }
            format!("{}. {}\n   Хариулт: {khariu}", i + 1, q.asuult.trim())
        })
        .collect::<Vec<_>>()
        .join("\n");
    let asuultuud = if asuultuud.is_empty() {
        String::new()
    } else {
        format!("Хэрэглэгчээс асууж авсан хариулт:\n{asuultuud}\n\nЭдгээр хариулт нь ажлын тайлбартай зөрчилдвөл хариулт давамгайлна. Хариулсан зүйлийг дахин таамаглахгүй.\n")
    };
    format!("\n## Тодруулга\n{oilgolt}{asuultuud}")
}

/// Үр дүнг зураг болгож нүдээр шалгах журам. Загвар зураг хавсаргасан бол түүнтэй
/// таартал давтана — кодыг уншаад «таарсан» гэж дүгнэхээс сэргийлнэ.
fn kharagdakh_khesg(a: &Ajil) -> String {
    let dir = a.shalgalt_dir;
    let kharitsuulakh = if a.zurguud.is_empty() {
        String::new()
    } else {
        "5. Хавсаргасан загвар зурагтай хэсэг хэсгээр нь харьцуул: өнгө, фонт, үсгийн хэмжээ, зай, хүрээ, баганын өргөн, зэрэгцүүлэлт, текст. Туршилтын өгөгдөлд зураг дээрх бодит утгуудыг ашигла. Зөрүү бүрийг засаад зургаа дахин гаргаж, таартал давт.\n\nЗагвар зураг бол дизайны гол эх сурвалж. Тайлбарт тодорхой өөрөөр заагаагүй бол өөрийн дизайн зохиохгүй, зураг дээрхийг яг дага.\n".to_string()
    };
    format!(
        r#"
## Харагдах байдлыг шалгах (UI, дизайн, хэвлэх загварын ажилд заавал)
Кодыг уншсанаар харагдах байдлыг баталгаажуулахгүй, үр дүнг зураг болгож өөрөө хар.
1. Туршилтын script, түр файлаа зөвхөн `{dir}/` хавтаст бич. Repo дотор түр файл үүсгэхгүй.
2. Өөрчилсөн загвар, компонентыг бодит кодоор нь дуудаж HTML гарга. Project-ийн alias import ажиллахгүй бол эх файлыг энэ хавтаст хуулж, import-ийг нь сольж ашигла.
3. Зураг гарга: `import {{ htmlZuragBolgokh }} from "./zuragBolgokh.mjs"; await htmlZuragBolgokh(html, "{dir}/ur.png", {{ urgun: 794, undur: 1123 }});`
4. `node {dir}/<script>.mjs` гэж яг энэ хэлбэрээр ажиллуулаад, гарсан PNG-г Read tool-оор нээж хар.
{kharitsuulakh}
Зураг гаргаж чадаагүй бол тайланд «нүдээр шалгаагүй» гэж шалтгаантай нь бич.
"#
    )
}

/// Өмнөх ээлжийн заавар, үр дүн
#[derive(Debug, Default, Clone, PartialEq)]
pub struct UmnukhEelj {
    /// Хоосон бол анхны тайлбараар ажилласан
    pub zaavar: String,
    pub ur: String,
    pub aldaa: String,
}

/// Сүүлийн ээлжийн тайланг бүтнээр нь шахам, өмнөхүүдийг товчилно — prompt хэт томрохгүй.
const SUULIIN_TAILAN_MAX: usize = 8000;
const UMNUKH_TAILAN_MAX: usize = 2000;

/// Мөр хадгалан тэмдэгтээр таслана.
fn taslakh(s: &str, n: usize) -> String {
    let s = s.trim();
    if s.chars().count() > n {
        format!("{}\n…(таслав)", s.chars().take(n).collect::<String>())
    } else {
        s.to_string()
    }
}

fn tuukh_khesg(a: &Ajil) -> String {
    if a.tuukh.is_empty() {
        return String::new();
    }
    let suuliin = a.tuukh.len() - 1;
    let eeljuud = a
        .tuukh
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let zaavar = if e.zaavar.trim().is_empty() {
                "Анхны тайлбараар (дээрх «Ажлын тайлбар»)".to_string()
            } else {
                format!("Нэмэлт заавар:\n{}", e.zaavar.trim())
            };
            let max = if i == suuliin {
                SUULIIN_TAILAN_MAX
            } else {
                UMNUKH_TAILAN_MAX
            };
            let ur = if e.ur.trim().is_empty() {
                "(тайлан алга)".to_string()
            } else {
                taslakh(&e.ur, max)
            };
            let aldaa = if e.aldaa.trim().is_empty() {
                String::new()
            } else {
                format!("\nАлдаа: {}", e.aldaa.trim())
            };
            format!("### Ээлж {}\n{zaavar}{aldaa}\n\nТайлан:\n{ur}", i + 1)
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    format!(
        "\n## Өмнөх ээлжүүд\nЭнэ ажил дээр өмнө нь {} удаа ажилласан. Хэрэглэгч үр дүнг хараад доорх нэмэлт заавар өгсөн.\n\n{eeljuud}\n",
        a.tuukh.len()
    )
}

pub struct ClaudeUr {
    pub zardal_usd: Option<f64>,
    /// `--json-schema`-ийн `structured_output` (тайлан, асуулт). Түүхий `result`
    /// текстийг зөвхөн алдааны мессежид ашиглана.
    pub butets: Option<Value>,
}

pub fn tovch(s: &str, n: usize) -> String {
    let s = s.replace(['\n', '\r'], " ");
    if s.chars().count() > n {
        format!("{}…", s.chars().take(n).collect::<String>())
    } else {
        s
    }
}

/// stream-json-ийн нэг мөрийг явцын мөр болгоно (assistant-ийн текст, tool дуудлага).
fn yavts_mur(v: &Value) -> Vec<String> {
    if v.get("type").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }
    let content = v.pointer("/message/content").and_then(Value::as_array);
    content
        .into_iter()
        .flatten()
        .filter_map(|c| match c.get("type").and_then(Value::as_str) {
            Some("text") => c
                .get("text")
                .and_then(Value::as_str)
                .filter(|t| !t.trim().is_empty())
                .map(|t| format!("💬 {}", tovch(t, 160))),
            Some("tool_use") => {
                let ner = c.get("name").and_then(Value::as_str).unwrap_or("?");
                let input = c.get("input");
                let s = |k: &str| input.and_then(|i| i.get(k)).and_then(Value::as_str);
                let delgerengui = s("file_path")
                    .or_else(|| s("command"))
                    .or_else(|| s("pattern"))
                    .or_else(|| s("path"))
                    .unwrap_or("");
                Some(format!("🔧 {ner} {}", tovch(delgerengui, 140)))
            }
            _ => None,
        })
        .collect()
}

/// Service-ээс git ажиллуулна. Credential prompt-д service гацахгүйн тулд
/// интерактив асуултыг хаана; timeout хэтэрвэл процессыг зогсооно.
pub(crate) async fn git(repo: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GCM_INTERACTIVE", "never")
        .stdin(Stdio::null())
        .kill_on_drop(true);
    let out = tokio::time::timeout(timeout, cmd.output())
        .await
        .map_err(|_| {
            format!(
                "git {} {} сек-д дуусаагүй",
                args.join(" "),
                timeout.as_secs()
            )
        })?
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

pub async fn git_status(repo: &str) -> Result<String, String> {
    git(repo, &["status", "--porcelain"], Duration::from_secs(60)).await
}

#[derive(Debug, PartialEq)]
pub enum PullUr {
    /// Branch-д upstream (tracking) тохируулаагүй — шинэчлэх эх алга
    UpstreamGui,
    /// Аль хэдийн шинэ
    Shine,
    /// Татсан commit-ийн тоо
    Tatsan(u64),
}

/// `git pull --ff-only`. Fast-forward боломжгүй (local commit-той салаалсан,
/// эсвэл commit хийгдээгүй өөрчлөлт татах файлтай давхцсан) бол git юу ч
/// өөрчлөхгүйгээр зогсож Err буцаана — working tree хэзээ ч эвдрэхгүй.
pub async fn git_pull(repo: &str) -> Result<PullUr, String> {
    let bogino = Duration::from_secs(30);
    if git(
        repo,
        &["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"],
        bogino,
    )
    .await
    .is_err()
    {
        return Ok(PullUr::UpstreamGui);
    }
    let umnukh = git(repo, &["rev-parse", "HEAD"], bogino)
        .await?
        .trim()
        .to_string();
    git(
        repo,
        &["pull", "--ff-only", "--no-rebase"],
        Duration::from_secs(180),
    )
    .await?;
    let daraa = git(repo, &["rev-parse", "HEAD"], bogino)
        .await?
        .trim()
        .to_string();
    if umnukh == daraa {
        return Ok(PullUr::Shine);
    }
    let too = git(
        repo,
        &["rev-list", "--count", &format!("{umnukh}..{daraa}")],
        bogino,
    )
    .await?
    .trim()
    .parse()
    .unwrap_or(0);
    Ok(PullUr::Tatsan(too))
}

/// Ажлын агуулга, repo, өмнөх ээлж — ажлын болон асуултын prompt хоёуланд ижил.
fn aguulga_khesg(a: &Ajil, bokhir: &[String]) -> String {
    let repo_jagsaalt = a
        .repos
        .iter()
        .map(|(ner, zam)| format!("- {ner}: {zam}"))
        .collect::<Vec<_>>()
        .join("\n");
    let zurguud = if a.zurguud.is_empty() {
        String::new()
    } else {
        let murnuud = a
            .zurguud
            .iter()
            .enumerate()
            .map(|(i, (zam, tailbar))| {
                if tailbar.trim().is_empty() {
                    format!("{}. {zam}", i + 1)
                } else {
                    format!("{}. {zam} — {}", i + 1, tailbar.trim())
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        format!(
            "\n## Хавсаргасан зураг\nRead tool-оор нээж харна (screenshot, алдааны дэлгэц, загвар г.м.):\n{murnuud}\n"
        )
    };
    let system = if a.system.trim().is_empty() {
        String::new()
    } else {
        format!("\nСистем: {}\n", a.system.trim())
    };
    let baikhgui = if a.baikhgui.is_empty() {
        String::new()
    } else {
        format!(
            "\nСистемийн дараах repo энэ компьютер дээр байхгүй тул засах боломжгүй — тэнд өөрчлөлт хэрэгтэй бол тайланд тэмдэглэ: {}\n",
            a.baikhgui.join(", ")
        )
    };
    let bokhir = if bokhir.is_empty() {
        String::new()
    } else {
        format!(
            "\nДараах repo-д өмнөх commit хийгдээгүй өөрчлөлт байгаа — тэдгээрийг хэрэглэгчийнх гэж үзэж буцаахгүй, дарж бичихгүй: {}\n",
            bokhir.join(", ")
        )
    };
    let tuukh = tuukh_khesg(a);
    let nemelt = if a.nemelt_zaavar.trim().is_empty() {
        String::new()
    } else {
        format!(
            "\n## Энэ ээлжийн нэмэлт заавар (ГОЛ ДААЛГАВАР)\n{}\n\nӨмнөх ээлжид ажлыг дутуу эсвэл буруу ойлгосон байж болзошгүй. Энэ заавар анхны тайлбар, өмнөх тайлантай зөрчилдвөл энэ заавар давамгайлна.\n",
            a.nemelt_zaavar.trim()
        )
    };
    let umnukh_uurchlult = if a.umnukh_uurchlult.is_empty() {
        String::new()
    } else {
        format!(
            "\nӨмнөх ээлжид чиний хийсэн өөрчлөлт commit хийгдээгүй байгаа repo: {} — `git diff`-ээр харж, түүн дээрээ үргэлжлүүл. Нэмэлт заавар шаардсан үед л тэдгээрийг өөрчил/буцаа.\n",
            a.umnukh_uurchlult.join(", ")
        )
    };
    format!(
        r#"# {garchig}
{system}
## Ажлын тайлбар
{tailbar}
{zurguud}{tuukh}{nemelt}
## Засвар хийж болох repo
Эдгээр нь тухайн системийн бүх project — ажилд хамаарахыг нь олж засна, бусдыг нь хөндөхгүй.
{repo_jagsaalt}
{baikhgui}{umnukh_uurchlult}{bokhir}"#,
        garchig = a.garchig.trim(),
        tailbar = a.tailbar.trim(),
    )
}

/// Хэрэглэгчээс хэзээ, хэрхэн асуух журам (`asuukh` үед)
fn asuukh_khesg(a: &Ajil) -> String {
    if !a.asuukh {
        return "\n## Асуулт\nЭнэ ажилд хэрэглэгчээс асуухгүй — тодорхойгүй зүйлд хамгийн боломжит тайлбарыг сонгож ажлаа бүрэн гүйцэтгэ, `asuultuud`-ийг хоосон массив болго.\n".into();
    }
    format!(
        r#"
## Хэрэглэгчээс асуух (зөвхөн зайлшгүй үед)
Ажлыг шууд гүйцэтгэ. Ажил тодорхой бол юу ч асуухгүйгээр бүрэн хийж дуусга — энэ нь ердийн тохиолдол.
Харин кодоос тодорхойлох боломжгүй, хэрэглэгчийн шийдэх ёстой зүйлтэй тулгарвал (бизнесийн дүрэм, хэд хэдэн зөв хувилбартай шийдэл, хамрах хүрээ, UI-ийн байрлал, өгөгдлийн бүтэц, буруу сонговол их засвар шаардах зүйл) таамаглахгүй, зогсоод асуу:
- Кодыг судалсны дараа, аль болох засвар хийхээсээ ӨМНӨ асуу. Аль хэдийн хийсэн засвараа хэвээр үлдээ — тайландаа бич, хариулт ирэхэд түүн дээрээ үргэлжлүүлнэ.
- Нэг дор {ASUULT_MAX}-өөс ихгүй асуулт. Асуулт бүрт 2-4 тодорхой, бие биенээ үгүйсгэсэн сонголт, сонголт бүрийн үр дагаврыг `tailbar`-т бич. Санал болгох сонголтоо эхэнд тавьж `sanal: true` болго. «Бусад» сонголт бүү нэм (хэрэглэгч чөлөөт хариулт бичиж чадна).
- Кодоос олж болох зүйл, ердийн инженерийн сонголт (нэршил, файлын байршил, тест), тайлбарт эсвэл «Тодруулга»-д хариулагдсан зүйлийг БҮҮ асуу.
- Асуулт, сонголтыг монголоор, бие даан ойлгогдохоор (контексттой) бич.
- Асуухдаа `tailan`-д юу олсон (холбогдох файл, одоогийн бүтэц), юу хийсэн, яагаад зогссоноо бич — хариулт ирэхэд энэ тэмдэглэлээс үргэлжлүүлнэ.
"#
    )
}

pub fn prompt_beltgey(a: &Ajil, bokhir: &[String]) -> String {
    let aguulga = aguulga_khesg(a, bokhir);
    let todruulga = todruulga_khesg(a);
    let asuukh = asuukh_khesg(a);
    let kharagdakh = kharagdakh_khesg(a);
    let journam1 = if a.tuukh.is_empty() {
        "Тайлбар, зургийг бүрэн уншиж юу хүсэж байгааг ойлго."
    } else {
        "Нэмэлт заавар, өмнөх тайлан, одоогийн `git diff`-ийг харьцуулж өмнө нь юуг буруу/дутуу ойлгосныг тодорхойл."
    };
    let journam2 = match (a.asuultuud.is_empty(), a.asuukh) {
        (true, false) => "Тодорхойгүй зүйлд хамгийн боломжит тайлбарыг сонгож, түүнийгээ тайланд тэмдэглэ.",
        (true, true) => "Жижиг тодорхойгүй зүйлд хамгийн боломжит тайлбарыг сонгож тайланд тэмдэглэ; хэрэглэгчийн шийдэх ёстой зүйл бол «Хэрэглэгчээс асуух» журмаар асуу.",
        (false, _) => "«Тодруулга» хэсгийн хариултыг дага. Түүнээс гадуурх тодорхойгүй зүйлд хамгийн боломжит тайлбарыг сонгож, тайланд тэмдэглэ.",
    };
    format!(
        r#"Чи Zevtabs-ийн хөгжүүлэгчийн turees системийн «Claude ажил» цонхонд бичсэн ажлыг гүйцэтгэж байна.

{aguulga}{todruulga}{asuukh}{kharagdakh}
Журам:
1. {journam1} Холбогдох кодыг олж, одоогийн бүтэц, нэршил, хэв маягийг дагаж өөрчил.
2. {journam2} Хүссэнээс хэтэрсэн рефактор хийхгүй.
3. Холбогдох тест, lint, type check, build-ийг боломжтой бол ажиллуул. Унасан бол унасан гэж бич, давсан гэж бүү хэл.
4. Bash команд бүрийг `cd`, `&&`, `;`, `|`-гүй ганц команд болгож ажиллуул, нийлмэл команд автоматаар татгалзагдана. Өөр repo-г `git -C <зам> status` хэлбэрээр шалга.

Хатуу дүрэм:
- git commit, push, deploy, production service restart хийхгүй. Өөрчлөлт зөвхөн local working tree-д үлдэнэ.
- Бодит төлбөр (qpay), ebarimt, SMS, бусад гадаад side-effect үйлчилгээг тестэд дуудахгүй.
- Production өгөгдлийг өөрчлөхгүй.
- Credential, token, харилцагчийн мэдээллийг тайланд бүү оруул.

Эцэст нь хариуг өгөгдсөн JSON schema-аар буцаа. `tailan`-д дараах бүтэцтэй Markdown тайлан бич
(энэ нь цонхонд харагдана; нэмэлт ээлж бол энэ ээлжид хийснийг, «Хийсэн ажил»-ын эхэнд өмнө нь
юуг буруу/дутуу ойлгосноо бич). Ажил дууссан бол `asuultuud` хоосон массив:
# {garchig}
## Хийсэн ажил
## Өөрчилсөн файлууд
## Тест / lint / build үр дүн
## Тодорхойгүй байсан зүйл, хийсэн таамаглал
## Үлдсэн ажил, зөвлөмж
"#,
        garchig = a.garchig.trim(),
    )
}

/// `claude -p`-ийн аргументууд. Тестлэгдэхүйц байлгах үүднээс процесс
/// эхлүүлэхээс тусад нь бэлдэнэ.
fn claude_args(t: &ClaudeTokhirgoo, shalgalt_dir: &str, asuukh: bool) -> Vec<String> {
    let mut v: Vec<String> = [
        "-p",
        "--output-format",
        "stream-json",
        "--verbose",
        "--permission-mode",
        "acceptEdits",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    v.extend(["--max-budget-usd".into(), t.budget_usd.to_string()]);
    v.extend(["--allowedTools".into(), zuvshuurul_jagsaalt(shalgalt_dir).join(",")]);
    v.extend(["--disallowedTools".into(), khorig_jagsaalt().join(",")]);
    v.extend(["--json-schema".into(), ajil_schema(asuukh).to_string()]);
    v
}

/// Claude-г ажиллуулж эцсийн тайланг буцаана. Бүх stream event-ийг stream_path-д
/// аудитад хадгалж, явцын мөр бүрийг yavts-аар дамжуулна.
pub async fn ajilluulya(
    t: &ClaudeTokhirgoo,
    asuukh: bool,
    prompt: &str,
    repos: &[String],
    ajil_dir: &Path,
    stream_path: &Path,
    yavts: impl Fn(&str),
) -> Result<ClaudeUr, String> {
    let ekhnii = repos.first().ok_or("repo алга")?;
    let timeout_min = t.timeout_min;
    let mut cmd = Command::new(&t.exe);
    cmd.current_dir(ekhnii)
        .args(claude_args(t, &shalgalt_zam(ajil_dir)?, asuukh));
    if !t.model.is_empty() {
        cmd.arg("--model").arg(&t.model);
    }
    for dir in repos.iter().skip(1) {
        cmd.arg("--add-dir").arg(dir);
    }
    // Зургууд энд татагдсан
    cmd.arg("--add-dir").arg(ajil_dir);
    // Анхдагчаар зөвхөн cwd (эхний repo)-ийн CLAUDE.md ачаалагдана — --add-dir
    // repo-уудын CLAUDE.md-г ч ачаалуулж системийн мэдлэгийг бүрэн өгнө.
    cmd.env("CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD", "1");
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("'{}' ажиллуулж чадсангүй: {e}", t.exe))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|e| format!("stdin: {e}"))?;
        // stdin хаагдсанаар Claude prompt дууссаныг мэднэ
    }
    let stdout = child.stdout.take().ok_or("stdout алга")?;
    let mut stderr = child.stderr.take().ok_or("stderr алга")?;

    let mut stream_file = std::fs::File::create(stream_path)
        .map_err(|e| format!("{}: {e}", stream_path.display()))?;
    let mut ur_text: Option<String> = None;
    let mut zardal_usd = None;
    let mut aldaa_ur = false;
    let mut butets: Option<Value> = None;
    let unshilt = async {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = std::io::Write::write_all(&mut stream_file, format!("{line}\n").as_bytes());
            let Ok(v) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            for m in yavts_mur(&v) {
                yavts(&m);
            }
            if v.get("type").and_then(Value::as_str) == Some("result") {
                ur_text = v.get("result").and_then(Value::as_str).map(str::to_string);
                zardal_usd = v.get("total_cost_usd").and_then(Value::as_f64);
                aldaa_ur = v.get("is_error").and_then(Value::as_bool) == Some(true);
                butets = v.get("structured_output").filter(|s| !s.is_null()).cloned();
            }
        }
        child.wait().await
    };
    let status = tokio::time::timeout(Duration::from_secs(timeout_min.max(1) * 60), unshilt)
        .await
        .map_err(|_| format!("Claude {timeout_min} минутад дуусаагүй — зогсоолоо"))?
        .map_err(|e| format!("Claude: {e}"))?;

    let ur = ur_text.unwrap_or_default();
    if !status.success() || aldaa_ur {
        let mut err = String::new();
        let _ = stderr.read_to_string(&mut err).await;
        let tailbar = if err.trim().is_empty() {
            tovch(&ur, 300)
        } else {
            tovch(&err, 500)
        };
        return Err(format!("Claude exit {:?}: {tailbar}", status.code()));
    }
    Ok(ClaudeUr { zardal_usd, butets })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sh_git(dir: &Path, args: &[&str]) {
        let st = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args([
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@t",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&st.stderr)
        );
    }

    /// origin (bare) + clone + origin-д шинэ commit түлхэх туслах clone
    fn git_orchin(ner: &str) -> (PathBuf, PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("rcn-pull-{ner}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let origin = root.join("origin.git");
        let a = root.join("a");
        let b = root.join("b");
        sh_git(
            &root,
            &["init", "--bare", "-b", "main", origin.to_str().unwrap()],
        );
        sh_git(
            &root,
            &["clone", origin.to_str().unwrap(), a.to_str().unwrap()],
        );
        std::fs::write(a.join("f.txt"), "1\n").unwrap();
        sh_git(&a, &["checkout", "-b", "main"]);
        sh_git(&a, &["add", "."]);
        sh_git(&a, &["commit", "-m", "1"]);
        sh_git(&a, &["push", "-u", "origin", "main"]);
        sh_git(
            &root,
            &["clone", origin.to_str().unwrap(), b.to_str().unwrap()],
        );
        (root, a, b)
    }

    #[tokio::test]
    async fn git_pull_tatna_shine_upstreamgui() {
        let (root, a, b) = git_orchin("ok");
        let bs = b.to_str().unwrap();
        assert_eq!(git_pull(bs).await, Ok(PullUr::Shine));

        std::fs::write(a.join("f.txt"), "2\n").unwrap();
        sh_git(&a, &["commit", "-am", "2"]);
        std::fs::write(a.join("g.txt"), "x\n").unwrap();
        sh_git(&a, &["add", "."]);
        sh_git(&a, &["commit", "-m", "3"]);
        sh_git(&a, &["push"]);
        assert_eq!(git_pull(bs).await, Ok(PullUr::Tatsan(2)));
        // core.autocrlf=true үед checkout CRLF болгодог тул мөрийн төгсгөлийг үл хамаарна
        assert_eq!(
            std::fs::read_to_string(b.join("f.txt")).unwrap().trim_end(),
            "2"
        );

        sh_git(&b, &["checkout", "-b", "local-only"]);
        assert_eq!(git_pull(bs).await, Ok(PullUr::UpstreamGui));
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn git_pull_zurchiltei_bol_uurchlultiig_khadgalna() {
        let (root, a, b) = git_orchin("zurchil");
        std::fs::write(a.join("f.txt"), "upstream\n").unwrap();
        sh_git(&a, &["commit", "-am", "2"]);
        sh_git(&a, &["push"]);
        // b-д commit хийгдээгүй (Claude-ийн) өөрчлөлт ижил файл дээр
        std::fs::write(b.join("f.txt"), "claude\n").unwrap();
        assert!(git_pull(b.to_str().unwrap()).await.is_err());
        assert_eq!(
            std::fs::read_to_string(b.join("f.txt")).unwrap(),
            "claude\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn prompt_dureem_aguulna() {
        let repos = [("tureesBack".to_string(), "D:/Work/tureesBack".to_string())];
        let zurguud = [(
            "D:/data/a/zurag-1.png".to_string(),
            "алдааны дэлгэц".to_string(),
        )];
        let baikhgui = ["quickqpaypack (D:/Work/quickqpaypack хавтас алга)".to_string()];
        let p = prompt_beltgey(
            &Ajil {
                garchig: "QPay засвар",
                tailbar: "qpayShalgay 500 өгч байна",
                system: "Zogsool",
                repos: &repos,
                baikhgui: &baikhgui,
                zurguud: &zurguud,
                nemelt_zaavar: "",
                tuukh: &[],
                umnukh_uurchlult: &[],
                shalgalt_dir: "D:/data/a/shalgalt",
                asuukh: true,
                oilgolt: "",
                asuultuud: &[],
            },
            &["D:/Work/turees".into()],
        );
        // Загвар зурагтай ажил үр дүнгээ зураг болгож харьцуулна
        assert!(p.contains("node D:/data/a/shalgalt/<script>.mjs"));
        // Windows-ийн үнэмлэхүй зам ESM import-д ажиллахгүй тул харьцангуй байна
        assert!(p.contains("from \"./zuragBolgokh.mjs\"") && !p.contains("from \"D:/"));
        assert!(p.contains("Хавсаргасан загвар зурагтай") && p.contains("таартал давт"));
        assert!(!p.contains("Өмнөх ээлжүүд") && !p.contains("ГОЛ ДААЛГАВАР"));
        assert!(p.contains("# QPay засвар") && p.contains("qpayShalgay 500"));
        assert!(p.contains("Систем: Zogsool"));
        assert!(p.contains("quickqpaypack (D:/Work/quickqpaypack хавтас алга)"));
        assert!(p.contains("tureesBack: D:/Work/tureesBack"));
        assert!(p.contains("zurag-1.png — алдааны дэлгэц"));
        assert!(p.contains("git commit, push, deploy"));
        assert!(p.contains("D:/Work/turees") && p.contains("буцаахгүй"));
    }

    #[test]
    fn zuraggui_bol_khesg_baikhgui() {
        let p = prompt_beltgey(
            &Ajil {
                garchig: "a",
                tailbar: "b",
                system: "",
                repos: &[],
                baikhgui: &[],
                zurguud: &[],
                nemelt_zaavar: "",
                tuukh: &[],
                umnukh_uurchlult: &[],
                shalgalt_dir: "D:/data/b/shalgalt",
                asuukh: true,
                oilgolt: "",
                asuultuud: &[],
            },
            &[],
        );
        assert!(!p.contains("Хавсаргасан загвар зурагтай"));
        assert!(!p.contains("Хавсаргасан зураг") && !p.contains("буцаахгүй"));
        assert!(!p.contains("Систем:") && !p.contains("байхгүй тул"));
    }

    #[test]
    fn nemelt_eelj_prompt() {
        let repos = [("parking-v2".to_string(), "D:/Work/parking-v2".to_string())];
        let urt = "x".repeat(UMNUKH_TAILAN_MAX + 100);
        let tuukh = [
            UmnukhEelj {
                zaavar: String::new(),
                ur: urt.clone(),
                aldaa: String::new(),
            },
            UmnukhEelj {
                zaavar: "karte-г бүү хас".into(),
                ur: "## Хийсэн ажил\nsdkService засав".into(),
                aldaa: String::new(),
            },
        ];
        let p = prompt_beltgey(
            &Ajil {
                garchig: "хөнгөлөлт",
                tailbar: "анхны тайлбар",
                system: "Zogsool",
                repos: &repos,
                baikhgui: &[],
                zurguud: &[],
                nemelt_zaavar: "Үнэгүй машинд karte огт шалгахгүй",
                tuukh: &tuukh,
                umnukh_uurchlult: &["parking-v2".into()],
                shalgalt_dir: "D:/data/c/shalgalt",
                asuukh: true,
                oilgolt: "",
                asuultuud: &[],
            },
            &[],
        );
        assert!(p.contains("анхны тайлбар"));
        assert!(p.contains("## Өмнөх ээлжүүд") && p.contains("2 удаа"));
        assert!(p.contains("### Ээлж 1\nАнхны тайлбараар"));
        assert!(p.contains("### Ээлж 2\nНэмэлт заавар:\nkarte-г бүү хас"));
        assert!(p.contains("sdkService засав"));
        // Өмнөх (сүүлийн биш) тайлан товчлогдоно
        assert!(p.contains("…(таслав)") && !p.contains(&urt));
        assert!(p.contains("ГОЛ ДААЛГАВАР)\nҮнэгүй машинд karte огт шалгахгүй"));
        assert!(p.contains("commit хийгдээгүй байгаа repo: parking-v2"));
        // Нэмэлт заавар нь өмнөх ээжлийн дараа байрлана
        assert!(p.find("Өмнөх ээлжүүд").unwrap() < p.find("ГОЛ ДААЛГАВАР").unwrap());
    }

    #[test]
    fn stream_json_yavts() {
        let v = serde_json::json!({"type": "assistant", "message": {"content": [
            {"type": "text", "text": "qpayShalgay-г шалгая"},
            {"type": "tool_use", "name": "Read", "input": {"file_path": "D:/Work/tureesBack/routes/qpayRoute.js"}},
            {"type": "tool_use", "name": "Bash", "input": {"command": "node --test test/"}}
        ]}});
        let m = yavts_mur(&v);
        assert_eq!(m.len(), 3);
        assert!(m[1].contains("Read") && m[1].contains("qpayRoute.js"));
        assert!(m[2].contains("node --test"));
        assert!(yavts_mur(&serde_json::json!({"type": "user"})).is_empty());
    }

    #[test]
    fn khorig_ni_zuvshuurulduu_baikhgui() {
        let zuvshuurul = zuvshuurul_jagsaalt("D:/data/a/shalgalt");
        let khorig = khorig_jagsaalt();
        for k in &khorig {
            assert!(!zuvshuurul.contains(k), "{k}");
        }
        for k in [
            "Bash(git push:*)",
            "Bash(git commit:*)",
            "Bash(git -C * push)",
            "Bash(git -C * push *)",
            "Bash(git -C * commit *)",
        ] {
            assert!(khorig.iter().any(|x| x == k), "{k}");
        }
    }

    #[test]
    fn skill_script_zuvkhun_tuukhain_zamaar() {
        assert!(skill_script_erkh(None).is_empty());
        assert!(skill_script_erkh(Some("  ")).is_empty());
        let v = skill_script_erkh(Some("C:\\Users\\pc\\"));
        let zam = "C:/Users/pc/.claude/skills/ui-ux-pro-max/scripts/search.py";
        assert!(v.contains(&format!("Bash(python {zam})")));
        assert!(v.contains(&format!("Bash(python {zam} *)")));
        // Дурын python script ажиллуулах эрх нэмэгдээгүй
        assert!(v.iter().all(|x| x.contains(zam)), "{v:?}");
        assert!(ZUVSHUURUL.contains(&"Skill"));
    }

    #[test]
    fn node_erkh_zuvkhun_shalgalt_khavtsand() {
        let v = zuvshuurul_jagsaalt("D:/data/a/shalgalt");
        let node: Vec<_> = v.iter().filter(|x| x.starts_with("Bash(node ")).collect();
        assert!(node.iter().any(|x| *x == "Bash(node D:/data/a/shalgalt/*)"));
        // Дурын `node <файл>`, `node -e` эрх нэмэгдээгүй
        assert!(node
            .iter()
            .all(|x| x.starts_with("Bash(node --") || x.starts_with("Bash(node D:/data/a/shalgalt/")));
    }

    #[test]
    fn shalgalt_khavtas_tuslakhtai_beldene() {
        let root = std::env::temp_dir().join(format!("rcn-shalgalt-{}", std::process::id()));
        let zam = shalgalt_beltgey(&root).unwrap();
        assert!(!zam.contains('\\') && zam.ends_with("/shalgalt"));
        let js = std::fs::read_to_string(format!("{zam}/zuragBolgokh.mjs")).unwrap();
        assert!(js.contains("export async function htmlZuragBolgokh"));
        let _ = std::fs::remove_dir_all(root);
    }

    /// Туслах бодитоор ажиллаж PNG гаргана (Edge/Chrome, node шаардлагатай)
    #[test]
    fn zurag_bolgokh_tuslakh_png_gargana() {
        let root = std::env::temp_dir().join(format!("rcn-zurag-test-{}", std::process::id()));
        let zam = shalgalt_beltgey(&root).unwrap();
        let script = format!("{zam}/t.mjs");
        std::fs::write(
            &script,
            format!(
                "import {{ htmlZuragBolgokh }} from './zuragBolgokh.mjs';
await htmlZuragBolgokh('<h1>Нэхэмжлэл</h1>', '{zam}/ur.png', {{ urgun: 300, undur: 200 }});
"
            ),
        )
        .unwrap();
        let o = std::process::Command::new("node").arg(&script).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let png = std::fs::read(format!("{zam}/ur.png")).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn anhdagch_zagvar_opus() {
        assert_eq!(ClaudeTokhirgoo::default().model, "claude-opus-5-5");
        let t: ClaudeTokhirgoo = toml::from_str("budget_usd = 1.0").unwrap();
        assert_eq!(t.model, "claude-opus-5-5");
    }

    fn songolt(ner: &str, sanal: bool) -> Songolt {
        Songolt {
            ner: ner.into(),
            tailbar: format!("{ner} тайлбар"),
            sanal,
        }
    }

    #[test]
    fn ajliin_ur_tseverley_buruu_asuultyg_khayana() {
        let v = serde_json::json!({
            "tailan": "  ## Олсон зүйл\ngereeController.js  ",
            "asuultuud": [
                { "asuult": "Хуваарилалт?", "tovch": "Хуваах", "olonSongolt": false, "songoltuud": [
                    { "ner": "Тэнцүү", "tailbar": "нийт ÷ тоо", "sanal": true },
                    { "ner": "Тэнцүү", "tailbar": "давхардсан", "sanal": false },
                    { "ner": "Талбайгаар", "tailbar": "м²", "sanal": false }
                ]},
                // Ганц сонголттой — хэрэглэгч сонгох юмгүй
                { "asuult": "Ганц?", "tovch": "", "olonSongolt": false, "songoltuud": [
                    { "ner": "А", "tailbar": "", "sanal": true }
                ]},
                { "asuult": "  ", "tovch": "", "olonSongolt": false, "songoltuud": [
                    { "ner": "А", "tailbar": "", "sanal": true }, { "ner": "Б", "tailbar": "", "sanal": false }
                ]},
                // Claude хариулт/khariulsan бичсэн ч тооцохгүй
                { "asuult": "3?", "olonSongolt": true, "khariu": ["А"], "khariulsan": true, "songoltuud": [
                    { "ner": "А", "tailbar": "", "sanal": false }, { "ner": "Б", "tailbar": "", "sanal": false }
                ]},
                { "asuult": "4?", "songoltuud": [{ "ner": "А" }, { "ner": "Б" }] },
                { "asuult": "5?", "songoltuud": [{ "ner": "А" }, { "ner": "Б" }] },
                { "asuult": "6?", "songoltuud": [{ "ner": "А" }, { "ner": "Б" }] }
            ]
        });
        let a = ajliin_ur_tseverley(&v).unwrap();
        assert_eq!(a.tailan, "## Олсон зүйл\ngereeController.js");
        assert_eq!(a.asuultuud.len(), ASUULT_MAX);
        assert_eq!(a.asuultuud[0].songoltuud.len(), 2, "давхардсан сонголт хасагдана");
        assert!(a.asuultuud[0].songoltuud[0].sanal);
        assert_eq!(a.asuultuud[1].asuult, "3?");
        assert!(a.asuultuud[1].olon_songolt);
        assert!(a
            .asuultuud
            .iter()
            .all(|q| q.khariu.is_empty() && q.busad.is_empty() && !q.khariulsan));

        let duussan =
            ajliin_ur_tseverley(&serde_json::json!({ "tailan": "# Дууслаа", "asuultuud": [] })).unwrap();
        assert!(duussan.asuultuud.is_empty() && duussan.tailan == "# Дууслаа");
        assert!(ajliin_ur_tseverley(&serde_json::json!({ "asuultuud": "буруу" })).is_err());
    }

    fn ajil_jishee<'a>(asuukh: bool, asuultuud: &'a [Asuult], oilgolt: &'a str) -> Ajil<'a> {
        Ajil {
            garchig: "Хөргүүрийн цахилгаан",
            tailbar: "хөргүүрийн цахилгааныг гэрээнд хуваарил",
            system: "Turees",
            repos: &[],
            baikhgui: &[],
            zurguud: &[],
            nemelt_zaavar: "",
            tuukh: &[],
            umnukh_uurchlult: &[],
            shalgalt_dir: "D:/data/q/shalgalt",
            asuukh,
            oilgolt,
            asuultuud,
        }
    }

    #[test]
    fn prompt_shuud_khiigeed_zuvkhun_zailshgui_uyd_asuuna() {
        let p = prompt_beltgey(&ajil_jishee(true, &[], ""), &[]);
        assert!(p.contains("Ажлыг шууд гүйцэтгэ") && p.contains("зогсоод асуу"));
        assert!(p.contains("JSON schema") && p.contains("`tailan`-д"));
        assert!(p.contains("## Хийсэн ажил"));
        // Асуухыг унтраасан бол асуухгүй
        let p = prompt_beltgey(&ajil_jishee(false, &[], ""), &[]);
        assert!(p.contains("хэрэглэгчээс асуухгүй") && !p.contains("зогсоод асуу"));
    }

    #[test]
    fn ajliin_prompt_todruulgatai() {
        let asuultuud = [
            Asuult {
                asuult: "Хэрхэн хуваах вэ?".into(),
                songoltuud: vec![songolt("Тэнцүү", true), songolt("Талбайгаар", false)],
                khariu: vec!["Талбайгаар".into()],
                busad: "м² нь гэрээнээс".into(),
                khariulsan: true,
                ..Default::default()
            },
            Asuult {
                asuult: "Хязгаар?".into(),
                songoltuud: vec![songolt("5-9 сар", true), songolt("Анхааруулга", false)],
                khariulsan: true,
                ..Default::default()
            },
        ];
        let p = prompt_beltgey(&ajil_jishee(true, &asuultuud, "gereeController.js-д зардал"), &[]);
        assert!(p.contains("## Тодруулга") && p.contains("gereeController.js-д зардал"));
        assert!(p.contains("Хариулт: Талбайгаар (Талбайгаар тайлбар)\n   Нэмэлт тайлбар: м² нь гэрээнээс"));
        // Алгассан бол санал болгосныг ашиглана
        assert!(p.contains("санал болгосон «5-9 сар»"));
        assert!(p.contains("«Тодруулга» хэсгийн хариултыг дага"));
        assert!(p.find("## Тодруулга").unwrap() < p.find("Журам:").unwrap());

        let p = prompt_beltgey(&ajil_jishee(true, &[], ""), &[]);
        assert!(!p.contains("## Тодруулга") && !p.contains("«Тодруулга» хэсгийн"));
    }

    #[test]
    fn claude_args_schema_ba_erkh() {
        let t = ClaudeTokhirgoo::default();
        let utga = |a: &[String], k: &str| a.iter().position(|x| x == k).map(|i| a[i + 1].clone()).unwrap();
        let a = claude_args(&t, "D:/data/a/shalgalt", true);
        assert_eq!(utga(&a, "--permission-mode"), "acceptEdits");
        assert_eq!(utga(&a, "--max-budget-usd"), "5");
        let schema: Value = serde_json::from_str(&utga(&a, "--json-schema")).unwrap();
        assert_eq!(schema["properties"]["asuultuud"]["maxItems"], ASUULT_MAX);
        assert!(utga(&a, "--disallowedTools").contains("Bash(git push:*)"));
        // Асуухыг унтраасан бол schema асуулт зөвшөөрөхгүй
        let a = claude_args(&t, "D:/data/a/shalgalt", false);
        let schema: Value = serde_json::from_str(&utga(&a, "--json-schema")).unwrap();
        assert_eq!(schema["properties"]["asuultuud"]["maxItems"], 0);
    }

    async fn bodit_ajilluulya(tailbar: &str, ner: &str) -> (AjliinUr, String) {
        let fail = "function tsakhilgaanKhuvaari(niit, gereenuud) {\n  // TODO\n}\nmodule.exports = { tsakhilgaanKhuvaari };\n";
        let root = std::env::temp_dir().join(format!("rcn-bodit-{ner}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("tsakhilgaan.js"), fail).unwrap();
        let ajil_dir = root.join("ajil");
        let shalgalt = shalgalt_beltgey(&ajil_dir).unwrap();
        let repos = [("repo".to_string(), repo.display().to_string())];
        let a = Ajil {
            garchig: "Хөргүүрийн цахилгаан",
            tailbar,
            system: "",
            repos: &repos,
            baikhgui: &[],
            zurguud: &[],
            nemelt_zaavar: "",
            tuukh: &[],
            umnukh_uurchlult: &[],
            shalgalt_dir: &shalgalt,
            asuukh: true,
            oilgolt: "",
            asuultuud: &[],
        };
        let t = ClaudeTokhirgoo {
            exe: std::env::var("CLAUDE_EXE").unwrap_or_else(|_| "claude".into()),
            model: "claude-haiku-5-5".into(),
            budget_usd: 0.5,
            ..Default::default()
        };
        let ur = ajilluulya(
            &t,
            true,
            &prompt_beltgey(&a, &[]),
            &[repo.display().to_string()],
            &ajil_dir,
            &ajil_dir.join("claude.jsonl"),
            |m| println!("{m}"),
        )
        .await
        .unwrap();
        let ajliin_ur = ajliin_ur_tseverley(&ur.butets.expect("structured_output")).unwrap();
        println!("{ajliin_ur:#?} ${:?}", ur.zardal_usd);
        let daraa = std::fs::read_to_string(repo.join("tsakhilgaan.js")).unwrap();
        let _ = std::fs::remove_dir_all(root);
        (ajliin_ur, daraa)
    }

    /// Бодит `claude` CLI: тодорхой ажлыг асуулгүй хийнэ. Зардалтай тул гараар:
    /// `cargo test -- --ignored bodit_claude`
    #[tokio::test]
    #[ignore]
    async fn bodit_claude_todorkhoi_ajlyg_shuud_khiine() {
        let (ur, daraa) = bodit_ajilluulya(
            "tsakhilgaan.js-ийн tsakhilgaanKhuvaari-г нийт дүнг (niit) гэрээний тоонд (gereenuud.length) тэнцүү хувааж, гэрээ бүрийн дүнгийн массив буцаадаг болго.",
            "todorkhoi",
        )
        .await;
        assert!(ur.asuultuud.is_empty(), "тодорхой ажилд асуухгүй: {:?}", ur.asuultuud);
        assert!(!daraa.contains("// TODO"), "засвар хийгдсэн байх ёстой:\n{daraa}");
    }

    /// Бизнесийн шийдвэр шаардсан ажилд зогсоод асуумж буцаана.
    #[tokio::test]
    #[ignore]
    async fn bodit_claude_todorkhoigui_bol_asuuna() {
        let (ur, _) = bodit_ajilluulya(
            "tsakhilgaan.js-ийн tsakhilgaanKhuvaari-д хөргүүрийн нийт цахилгааныг гэрээнүүдэд хуваарилах логик бич. Ямар зарчмаар (тэнцүү, талбайгаар, тоолуураар) хуваахыг би хараахан шийдээгүй — энэ нь бизнесийн шийдвэр.",
            "todorkhoigui",
        )
        .await;
        assert!(!ur.asuultuud.is_empty(), "бизнесийн шийдвэрийг асуух ёстой");
        assert!(!ur.tailan.trim().is_empty());
    }
}
