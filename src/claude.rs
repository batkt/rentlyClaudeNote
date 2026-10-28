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
}

impl Default for ClaudeTokhirgoo {
    fn default() -> Self {
        Self {
            exe: default_exe(),
            budget_usd: default_budget(),
            timeout_min: default_timeout(),
            model: String::new(),
            bokhir_repo_zuvshuurukh: false,
        }
    }
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
    /// (repo нэр, local зам)
    pub repos: &'a [(String, String)],
    /// (local файлын зам, зургийн тайлбар)
    pub zurguud: &'a [(String, String)],
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

pub async fn git_status(repo: &str) -> Result<String, String> {
    let out = Command::new("git")
        .args(["-C", repo, "status", "--porcelain"])
        .output()
        .await
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
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
    let bokhir = if bokhir.is_empty() {
        String::new()
    } else {
        format!(
            "\nДараах repo-д өмнөх commit хийгдээгүй өөрчлөлт байгаа — тэдгээрийг хэрэглэгчийнх гэж үзэж буцаахгүй, дарж бичихгүй: {}\n",
            bokhir.join(", ")
        )
    };
    format!(
        r#"Чи Zevtabs-ийн хөгжүүлэгчийн turees системийн «Claude ажил» цонхонд бичсэн ажлыг гүйцэтгэж байна.

# {garchig}

## Ажлын тайлбар
{tailbar}
{zurguud}
## Засвар хийж болох repo
{repo_jagsaalt}
{bokhir}
Журам:
1. Тайлбар, зургийг бүрэн уншиж юу хүсэж байгааг ойлго. Холбогдох кодыг олж, одоогийн бүтэц, нэршил, хэв маягийг дагаж өөрчил.
2. Тодорхойгүй зүйлд хамгийн боломжит тайлбарыг сонгож, түүнийгээ тайланд тэмдэглэ. Хүссэнээс хэтэрсэн рефактор хийхгүй.
3. Холбогдох тест, lint, type check, build-ийг боломжтой бол ажиллуул. Унасан бол унасан гэж бич, давсан гэж бүү хэл.

Хатуу дүрэм:
- git commit, push, deploy, production service restart хийхгүй. Өөрчлөлт зөвхөн local working tree-д үлдэнэ.
- Бодит төлбөр (qpay), ebarimt, SMS, бусад гадаад side-effect үйлчилгээг тестэд дуудахгүй.
- Production өгөгдлийг өөрчлөхгүй.
- Credential, token, харилцагчийн мэдээллийг тайланд бүү оруул.

Эцэст нь дараах бүтэцтэй Markdown тайлан хэвлэ (энэ нь цонхонд харагдана):
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
        .args(["-p", "--output-format", "stream-json", "--verbose", "--permission-mode", "acceptEdits"])
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
    cmd.arg("--allowedTools").arg(ZUVSHUURUL.join(","));
    cmd.arg("--disallowedTools").arg(KHORIG.join(","));
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = cmd.spawn().map_err(|e| format!("'{}' ажиллуулж чадсангүй: {e}", t.exe))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin.write_all(prompt.as_bytes()).await.map_err(|e| format!("stdin: {e}"))?;
        // stdin хаагдсанаар Claude prompt дууссаныг мэднэ
    }
    let stdout = child.stdout.take().ok_or("stdout алга")?;
    let mut stderr = child.stderr.take().ok_or("stderr алга")?;

    let mut stream_file =
        std::fs::File::create(stream_path).map_err(|e| format!("{}: {e}", stream_path.display()))?;
    let mut ur_text: Option<String> = None;
    let mut zardal_usd = None;
    let mut aldaa_ur = false;
    let unshilt = async {
        let mut lines = BufReader::new(stdout).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let _ = std::io::Write::write_all(&mut stream_file, format!("{line}\n").as_bytes());
            let Ok(v) = serde_json::from_str::<Value>(&line) else { continue };
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
        let tailbar = if err.trim().is_empty() { tovch(&ur, 300) } else { tovch(&err, 500) };
        return Err(format!("Claude exit {:?}: {tailbar}", status.code()));
    }
    Ok(ClaudeUr { ur, zardal_usd })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_dureem_aguulna() {
        let repos = [("tureesBack".to_string(), "D:/Work/tureesBack".to_string())];
        let zurguud = [("D:/data/a/zurag-1.png".to_string(), "алдааны дэлгэц".to_string())];
        let p = prompt_beltgey(
            &Ajil { garchig: "QPay засвар", tailbar: "qpayShalgay 500 өгч байна", repos: &repos, zurguud: &zurguud },
            &["D:/Work/turees".into()],
        );
        assert!(p.contains("# QPay засвар") && p.contains("qpayShalgay 500"));
        assert!(p.contains("tureesBack: D:/Work/tureesBack"));
        assert!(p.contains("zurag-1.png — алдааны дэлгэц"));
        assert!(p.contains("git commit, push, deploy"));
        assert!(p.contains("D:/Work/turees") && p.contains("буцаахгүй"));
    }

    #[test]
    fn zuraggui_bol_khesg_baikhgui() {
        let p = prompt_beltgey(&Ajil { garchig: "a", tailbar: "b", repos: &[], zurguud: &[] }, &[]);
        assert!(!p.contains("Хавсаргасан зураг") && !p.contains("буцаахгүй"));
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
