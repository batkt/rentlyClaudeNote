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
    "Bash(git commit:*)",
    "Bash(git push:*)",
    "Bash(git reset:*)",
    "Bash(git checkout:*)",
    "Bash(git restore:*)",
    "Bash(git clean:*)",
    "Bash(git stash:*)",
    "Bash(git rebase:*)",
    "Bash(git merge:*)",
    "Bash(npm install:*)",
    "Bash(npm publish:*)",
    "Bash(pm2:*)",
    "WebFetch",
    "WebSearch",
];

#[derive(Deserialize, Clone)]
pub struct ClaudeTokhirgoo {
    #[serde(default = "default_exe")]
    pub exe: String,
    #[serde(default = "default_budget")]
    pub budget_usd: f64,
    #[serde(default = "default_timeout")]
    pub timeout_min: u64,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub bokhir_repo_zuvshuurukh: bool,
    /// Ажил эхлэхийн өмнө repo бүрийг `git pull --ff-only`-оор шинэчилнэ —
    /// Claude хуучирсан код дээр засаад дараа нь merge зөрчил үүсгэхээс сэргийлнэ.
    #[serde(default = "default_true")]
    pub git_pull: bool,
}

impl Default for ClaudeTokhirgoo {
    fn default() -> Self {
        Self {
            exe: default_exe(),
            budget_usd: default_budget(),
            timeout_min: default_timeout(),
            model: String::new(),
            bokhir_repo_zuvshuurukh: false,
            git_pull: true,
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_exe() -> String {
    "claude".into()
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
    pub ur: String,
    pub zardal_usd: Option<f64>,
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

pub fn prompt_beltgey(a: &Ajil, bokhir: &[String]) -> String {
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
    let journam1 = if a.tuukh.is_empty() {
        "Тайлбар, зургийг бүрэн уншиж юу хүсэж байгааг ойлго."
    } else {
        "Нэмэлт заавар, өмнөх тайлан, одоогийн `git diff`-ийг харьцуулж өмнө нь юуг буруу/дутуу ойлгосныг тодорхойл."
    };
    format!(
        r#"Чи Zevtabs-ийн хөгжүүлэгчийн turees системийн «Claude ажил» цонхонд бичсэн ажлыг гүйцэтгэж байна.

# {garchig}
{system}
## Ажлын тайлбар
{tailbar}
{zurguud}{tuukh}{nemelt}
## Засвар хийж болох repo
Эдгээр нь тухайн системийн бүх project — ажилд хамаарахыг нь олж засна, бусдыг нь хөндөхгүй.
{repo_jagsaalt}
{baikhgui}{umnukh_uurchlult}{bokhir}
Журам:
1. {journam1} Холбогдох кодыг олж, одоогийн бүтэц, нэршил, хэв маягийг дагаж өөрчил.
2. Тодорхойгүй зүйлд хамгийн боломжит тайлбарыг сонгож, түүнийгээ тайланд тэмдэглэ. Хүссэнээс хэтэрсэн рефактор хийхгүй.
3. Холбогдох тест, lint, type check, build-ийг боломжтой бол ажиллуул. Унасан бол унасан гэж бич, давсан гэж бүү хэл.

Хатуу дүрэм:
- git commit, push, deploy, production service restart хийхгүй. Өөрчлөлт зөвхөн local working tree-д үлдэнэ.
- Бодит төлбөр (qpay), ebarimt, SMS, бусад гадаад side-effect үйлчилгээг тестэд дуудахгүй.
- Production өгөгдлийг өөрчлөхгүй.
- Credential, token, харилцагчийн мэдээллийг тайланд бүү оруул.

Эцэст нь дараах бүтэцтэй Markdown тайлан хэвлэ (энэ нь цонхонд харагдана; нэмэлт ээлж бол
энэ ээлжид хийснийг, «Хийсэн ажил»-ын эхэнд өмнө нь юуг буруу/дутуу ойлгосноо бич):
# {garchig}
## Хийсэн ажил
## Өөрчилсөн файлууд
## Тест / lint / build үр дүн
## Тодорхойгүй байсан зүйл, хийсэн таамаглал
## Үлдсэн ажил, зөвлөмж
"#,
        garchig = a.garchig.trim(),
        tailbar = a.tailbar.trim(),
    )
}

/// Claude-г ажиллуулж эцсийн тайланг буцаана. Бүх stream event-ийг stream_path-д
/// аудитад хадгалж, явцын мөр бүрийг yavts-аар дамжуулна.
pub async fn ajilluulya(
    t: &ClaudeTokhirgoo,
    prompt: &str,
    repos: &[String],
    ajil_dir: &Path,
    stream_path: &Path,
    yavts: impl Fn(&str),
) -> Result<ClaudeUr, String> {
    let ekhnii = repos.first().ok_or("repo алга")?;
    let mut cmd = Command::new(&t.exe);
    cmd.current_dir(ekhnii)
        .args([
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "acceptEdits",
        ])
        .arg("--max-budget-usd")
        .arg(t.budget_usd.to_string());
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
    cmd.arg("--allowedTools").arg(ZUVSHUURUL.join(","));
    cmd.arg("--disallowedTools").arg(KHORIG.join(","));
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
            }
        }
        child.wait().await
    };
    let status = tokio::time::timeout(Duration::from_secs(t.timeout_min.max(1) * 60), unshilt)
        .await
        .map_err(|_| format!("Claude {} минутад дуусаагүй — зогсоолоо", t.timeout_min))?
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
    Ok(ClaudeUr { ur, zardal_usd })
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
            },
            &["D:/Work/turees".into()],
        );
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
            },
            &[],
        );
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
        for k in KHORIG {
            assert!(!ZUVSHUURUL.contains(k), "{k}");
        }
        assert!(KHORIG.contains(&"Bash(git push:*)") && KHORIG.contains(&"Bash(git commit:*)"));
    }
}
