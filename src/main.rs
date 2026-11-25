mod claude;
mod commit;
mod push;

use chrono::Local;
use reqwest::{Client, Method, StatusCode};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{sync::oneshot, time::sleep};
use windows_service::{
    define_windows_service,
    service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceErrorControl, ServiceExitCode,
        ServiceInfo, ServiceStartType, ServiceState, ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
    service_manager::{ServiceManager, ServiceManagerAccess},
};

const SERVICE_NAME: &str = "RentlyClaudeNote";
const SERVICE_DISPLAY_NAME: &str = "zevTurees Claude ажил гүйцэтгэгч";
/// Явцыг backend руу илгээх, цуцалсан эсэхийг шалгах давтамж
const YAVTS_SECS: u64 = 15;

// ─── Тохиргоо ─────────────────────────────────────────────────────────────────

// Debug derive хийхгүй — token, нууц үг log-д гарахаас сэргийлнэ.
#[derive(Deserialize, Clone)]
struct Config {
    #[serde(default = "default_url")]
    url: String,
    #[serde(default)]
    token: String,
    #[serde(default, rename = "nevtrekhNer")]
    nevtrekh_ner: String,
    #[serde(default, rename = "nuutsUg")]
    nuuts_ug: String,
    #[serde(default, rename = "baiguullagiinId")]
    baiguullagiin_id: String,
    #[serde(default = "default_shalgakh")]
    shalgakh_secs: u64,
    #[serde(default = "default_timeout")]
    timeout_secs: u64,
    /// Зэрэг ажиллах Claude ажлын дээд тоо. Repo нь давхцсан ажлууд хэзээ ч
    /// зэрэг ажиллахгүй — дараалалд (shine) хүлээнэ.
    #[serde(default = "default_zereg")]
    zereg_max: usize,
    /// Цонхны repo нэр → local зам (repo_undsen-ээс өөр газар байгаа repo-д)
    #[serde(default)]
    repos: BTreeMap<String, String>,
    /// [repos]-д байхгүй repo-г `<repo_undsen>/<нэр>`-ээс хайна (жишээ нь "D:/Work")
    #[serde(default)]
    repo_undsen: String,
    #[serde(default)]
    claude: claude::ClaudeTokhirgoo,
}

fn default_url() -> String {
    "https://turees.zevtabs.mn/api".into()
}
fn default_shalgakh() -> u64 {
    60
}
fn default_timeout() -> u64 {
    20
}
fn default_zereg() -> usize {
    1
}

// ─── Log ──────────────────────────────────────────────────────────────────────

type LogFile = Arc<Mutex<Box<dyn Write + Send>>>;

fn get_exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn open_log(dir: &Path) -> LogFile {
    let log_path = dir.join("rentlyClaudeNote.log");
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .expect("Log файл нээхэд алдаа");
    Arc::new(Mutex::new(Box::new(file)))
}

fn log_write(log: &LogFile, level: &str, msg: &str) {
    let line = format!(
        "[{}] [{}] {}\n",
        Local::now().format("%Y-%m-%d %H:%M:%S"),
        level,
        msg
    );
    print!("{}", line);
    if let Ok(mut f) = log.lock() {
        let _ = f.write_all(line.as_bytes());
    }
}

/// exe-ийн хажуудах config.toml, байхгүй бол одоогийн хавтасных (cargo run).
/// Буцаах хавтас нь log, data хадгалах үндсэн хавтас болно.
fn load_config() -> (Config, PathBuf) {
    let exe_dir = get_exe_dir();
    let candidates = [exe_dir.clone(), PathBuf::from(".")];
    let Some(dir) = candidates.iter().find(|d| d.join("config.toml").is_file()) else {
        eprintln!("config.toml олдсонгүй: {:?}", exe_dir.join("config.toml"));
        std::process::exit(1);
    };
    let config_str = fs::read_to_string(dir.join("config.toml")).unwrap_or_else(|e| {
        eprintln!("config.toml уншихад алдаа: {}", e);
        std::process::exit(1);
    });
    let mut config: Config = toml::from_str(&config_str).unwrap_or_else(|e| {
        eprintln!("config.toml алдаа: {}", e);
        std::process::exit(1);
    });
    config.url = config.url.trim_end_matches('/').to_string();
    (config, dir.clone())
}

fn khost() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into())
}

// ─── HTTP ─────────────────────────────────────────────────────────────────────

enum ApiAldaa {
    /// 401 — token дууссан эсвэл буруу
    Erkhgui,
    /// 409 — ажлын төлөв өөрчлөгдсөн (өөр service авсан, цуцалсан)
    Zurchil,
    Busad(String),
}

impl std::fmt::Display for ApiAldaa {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            ApiAldaa::Erkhgui => write!(f, "HTTP 401 — эрхгүй"),
            ApiAldaa::Zurchil => write!(f, "HTTP 409 — ажлын төлөв өөрчлөгдсөн"),
            ApiAldaa::Busad(s) => write!(f, "{s}"),
        }
    }
}

/// Зэрэг ажиллаж буй ажлууд хуваалцана — token-ийг Mutex-д хадгална.
struct Api {
    client: Client,
    config: Config,
    token: Mutex<Option<String>>,
}

fn khariu_tovch(body: &str) -> String {
    body.chars().take(300).collect()
}

impl Api {
    fn new(config: Config) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .user_agent("rentlyClaudeNote/0.1")
            .build()
            .expect("HTTP client үүсгэхэд алдаа");
        let token = Some(config.token.trim().to_string()).filter(|t| !t.is_empty());
        Self {
            client,
            config,
            token: Mutex::new(token),
        }
    }

    fn token_avya(&self) -> Option<String> {
        self.token.lock().ok().and_then(|t| t.clone())
    }

    fn token_solikh(&self, utga: Option<String>) {
        if let Ok(mut t) = self.token.lock() {
            *t = utga;
        }
    }

    async fn nevtrey(&self) -> Result<(), String> {
        if self.config.nevtrekh_ner.is_empty() || self.config.nuuts_ug.is_empty() {
            return Err(
                "token хүчингүй/байхгүй, config.toml-д nevtrekhNer, nuutsUg тохируулаагүй".into(),
            );
        }
        let mut body = json!({
            "nevtrekhNer": self.config.nevtrekh_ner,
            "nuutsUg": self.config.nuuts_ug,
        });
        if !self.config.baiguullagiin_id.is_empty() {
            body["baiguullagiinId"] = json!(self.config.baiguullagiin_id);
        }
        let resp = self
            .client
            .post(format!("{}/ajiltanNevtrey", self.config.url))
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("Нэвтрэх хүсэлт: {e}"))?;
        let status = resp.status();
        let text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(format!(
                "Нэвтрэх HTTP {}: {}",
                status.as_u16(),
                khariu_tovch(&text)
            ));
        }
        let v: Value = serde_json::from_str(&text).map_err(|e| format!("Нэвтрэх хариу: {e}"))?;
        if v.get("multipleOrgs").and_then(Value::as_bool) == Some(true) {
            return Err(
                "Олон байгууллагад бүртгэлтэй — config.toml-д baiguullagiinId тохируулна уу".into(),
            );
        }
        let token = v
            .get("token")
            .and_then(Value::as_str)
            .ok_or("Нэвтрэх хариунд token алга")?;
        self.token_solikh(Some(token.to_string()));
        Ok(())
    }

    async fn khuselt_once(
        &self,
        method: &Method,
        zam: &str,
        body: Option<&Value>,
    ) -> Result<Value, ApiAldaa> {
        let token = self.token_avya().unwrap_or_default();
        let mut req = self
            .client
            .request(method.clone(), format!("{}{}", self.config.url, zam))
            .bearer_auth(token);
        if let Some(b) = body {
            req = req.json(b);
        }
        let resp = req
            .send()
            .await
            .map_err(|e| ApiAldaa::Busad(e.to_string()))?;
        let status = resp.status();
        if status == StatusCode::UNAUTHORIZED {
            return Err(ApiAldaa::Erkhgui);
        }
        if status == StatusCode::CONFLICT {
            return Err(ApiAldaa::Zurchil);
        }
        let text = resp
            .text()
            .await
            .map_err(|e| ApiAldaa::Busad(e.to_string()))?;
        if status == StatusCode::FORBIDDEN {
            return Err(ApiAldaa::Busad(
                "HTTP 403 — Zevtabs CAdmin1 эрх шаардлагатай".into(),
            ));
        }
        if !status.is_success() {
            return Err(ApiAldaa::Busad(format!(
                "HTTP {}: {}",
                status.as_u16(),
                khariu_tovch(&text)
            )));
        }
        serde_json::from_str(&text).map_err(|e| ApiAldaa::Busad(format!("JSON: {e}")))
    }

    /// Token байхгүй эсвэл 401 бол нэг удаа дахин нэвтэрч оролдоно.
    async fn khuselt(
        &self,
        method: Method,
        zam: &str,
        body: Option<&Value>,
    ) -> Result<Value, ApiAldaa> {
        if self.token_avya().is_none() {
            self.nevtrey().await.map_err(ApiAldaa::Busad)?;
        }
        match self.khuselt_once(&method, zam, body).await {
            Err(ApiAldaa::Erkhgui) => {
                self.token_solikh(None);
                self.nevtrey().await.map_err(ApiAldaa::Busad)?;
                self.khuselt_once(&method, zam, body).await
            }
            r => r,
        }
    }

    async fn get(&self, zam: &str) -> Result<Value, ApiAldaa> {
        self.khuselt(Method::GET, zam, None).await
    }

    async fn post(&self, zam: &str, body: Value) -> Result<Value, ApiAldaa> {
        self.khuselt(Method::POST, zam, Some(&body)).await
    }

    /// /zuragAvya — зураг нь токен шаарддаггүй
    async fn zurag_tatay(&self, id: &str) -> Result<(Vec<u8>, String), String> {
        let resp = self
            .client
            .get(format!(
                "{}/zuragAvya/claudeAjil/{}/{}",
                self.config.url, self.config.baiguullagiin_id, id
            ))
            .send()
            .await
            .map_err(|e| format!("зураг {id}: {e}"))?;
        if !resp.status().is_success() {
            return Err(format!("зураг {id}: HTTP {}", resp.status().as_u16()));
        }
        let turul = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let bytes = resp.bytes().await.map_err(|e| format!("зураг {id}: {e}"))?;
        Ok((bytes.to_vec(), zurag_ext(&turul).to_string()))
    }
}

fn zurag_ext(content_type: &str) -> &'static str {
    match content_type.split(';').next().unwrap_or("").trim() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => "png",
    }
}

fn str_of<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// Repo нэрийг local зам болгоно: [repos]-д тохируулсан бол тэр, эс бөгөөс
/// `repo_undsen/<нэр>`. Нэр нь backend-ээс ирдэг тул хавтас дамжих нэрийг
/// (`..`, `/`, `\`) хүлээж авахгүй.
fn repo_zam(config: &Config, ner: &str) -> Option<String> {
    if let Some(zam) = config.repos.get(ner) {
        return Some(zam.clone());
    }
    if config.repo_undsen.trim().is_empty()
        || ner.is_empty()
        || ner.contains(['/', '\\'])
        || ner.contains("..")
    {
        return None;
    }
    Some(format!(
        "{}/{ner}",
        config.repo_undsen.trim().trim_end_matches(['/', '\\'])
    ))
}

/// Ажлын `repos` (backend-ийн системээс тодорхойлсон repo нэрүүд)
fn repo_nernuud(ajil: &Value) -> Vec<String> {
    ajil.get("repos")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// Repo түгжээний түлхүүр: local зам. Windows замд том жижиг үсэг, `\` / `/`
/// ялгаагүй тул нэг хэлбэрт оруулна — өөр нэрээр нэг хавтсыг заасан ч давхцлыг таньна.
fn repo_tulkhuur(zam: &str) -> String {
    zam.replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// Энэ компьютер дээр олдсон repo-уудын түгжээний түлхүүрүүд
fn lokal_repo_tulkhuuruud(config: &Config, ajil: &Value) -> Vec<String> {
    repo_nernuud(ajil)
        .iter()
        .filter_map(|ner| repo_zam(config, ner))
        .filter(|zam| Path::new(zam).is_dir())
        .map(|zam| repo_tulkhuur(&zam))
        .collect()
}

/// Дарааллын (хуучнаас нь эрэмбэлсэн) ажлуудаас одоо эхлүүлж болохуудын индекс.
/// Ажиллаж буй ажлын repo-той давхцсан ажил хүлээнэ. Хүлээж буй ажлын repo-г
/// мөн "захиалсан" гэж үзнэ — түүнтэй давхцах хойдох ажил урдуур орохгүй
/// (ижил repo-той ажлууд илгээсэн дарааллаараа гүйцэтгэгдэнэ).
fn songokh(jagsaalt: &[Vec<String>], ajillaj: &HashSet<String>, sul: usize) -> Vec<usize> {
    let mut zakhialsan = ajillaj.clone();
    let mut ur = Vec::new();
    for (i, repos) in jagsaalt.iter().enumerate() {
        if ur.len() >= sul {
            break;
        }
        let davkhtsana = repos.iter().any(|r| zakhialsan.contains(r));
        zakhialsan.extend(repos.iter().cloned());
        if !davkhtsana {
            ur.push(i);
        }
    }
    ur
}

type Ajillaj = Arc<Mutex<HashSet<String>>>;

/// Ажиллаж буй ажлын repo түгжээ. Drop болоход (ажил дууссан, алдаа гарсан,
/// service зогсоход abort хийгдсэн) repo-ууд автоматаар чөлөөлөгдөнө.
struct RepoTugjee {
    ajillaj: Ajillaj,
    repos: Vec<String>,
}

impl RepoTugjee {
    /// Repo-ууд чөлөөтэй болтол хүлээгээд түгжинэ. Төлөвлөгч давхцалгүйг
    /// шалгасан тул ихэвчлэн шууд түгжинэ; жагсаалт авах ба /avya-ийн хооронд
    /// хэрэглэгч систем сольсон ховор тохиолдолд л хүлээнэ.
    async fn avya(ajillaj: Ajillaj, repos: Vec<String>, log: &LogFile, id: &str) -> Self {
        let mut log_bichsen = false;
        loop {
            {
                let mut s = ajillaj.lock().unwrap_or_else(|e| e.into_inner());
                if !repos.iter().any(|r| s.contains(r)) {
                    s.extend(repos.iter().cloned());
                    break;
                }
            }
            if !log_bichsen {
                log_write(
                    log,
                    "WARN",
                    &format!("{id}: repo өөр ажилд ашиглагдаж байна — чөлөөлөгдөхийг хүлээнэ"),
                );
                log_bichsen = true;
            }
            sleep(Duration::from_secs(5)).await;
        }
        Self { ajillaj, repos }
    }
}

impl Drop for RepoTugjee {
    fn drop(&mut self) {
        let mut s = self.ajillaj.lock().unwrap_or_else(|e| e.into_inner());
        for r in &self.repos {
            s.remove(r);
        }
    }
}

/// `git status --porcelain` мөрүүдийг "<repo>: <мөр>" хэлбэрт оруулна. Backend-д
/// `zasvarlasan` болж хадгалагдах ба дараагийн ээлжид яг энэ хэлбэрээр харьцуулна.
fn status_murnuud(ner: &str, status: &str) -> Vec<String> {
    status
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| format!("{ner}: {}", l.trim()))
        .collect()
}

#[derive(Debug, PartialEq)]
enum RepoTuluv {
    Tseverkhen,
    /// Бүх өөрчлөлт нь өмнөх ээлжүүдэд Claude-ийн хийсэн өөрчлөлт
    UmnukhEeljiin,
    /// Хэрэглэгчийн (Claude-ийн бус) commit хийгдээгүй өөрчлөлт бий
    Bokhir,
}

fn angilakh(murnuud: &[String], umnukh: &HashSet<String>) -> RepoTuluv {
    if murnuud.is_empty() {
        RepoTuluv::Tseverkhen
    } else if murnuud.iter().all(|m| umnukh.contains(m)) {
        RepoTuluv::UmnukhEeljiin
    } else {
        RepoTuluv::Bokhir
    }
}

/// Backend-ийн `tuukh`-ээс өмнөх ээлжүүд ба тэдгээрийн бүх `zasvarlasan` мөрийн
/// нэгдлийг гаргана. Нэгдэл авах нь: дунд нэг ээлж эрт алдаа (жишээ нь bokhir)
/// гарч zasvarlasan хоосон үлдсэн ч өмнөх ээлжийн өөрчлөлтийг таньсаар байна.
/// Ажлын өөрийн `zasvarlasan`-ийг ч нэмнэ: Claude асуулт тавьж зогсохдоо хийсэн
/// өөрчлөлт энд хадгалагдана (хариулахад хэвээр). Дахин ажиллуулах, нэмэлт
/// ээлжид backend үүнийг хоослодог тул бусад үед хоосон.
fn tuukh_unshikh(ajil: &Value) -> (Vec<claude::UmnukhEelj>, HashSet<String>) {
    let mut murnuud: HashSet<String> = ajil
        .get("zasvarlasan")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    let eeljuud = ajil
        .get("tuukh")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|e| {
            murnuud.extend(
                e.get("zasvarlasan")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string),
            );
            claude::UmnukhEelj {
                zaavar: str_of(e, "zaavar").to_string(),
                ur: str_of(e, "ur").to_string(),
                aldaa: str_of(e, "aldaa").to_string(),
            }
        })
        .collect();
    (eeljuud, murnuud)
}

// ─── Ажил гүйцэтгэх ───────────────────────────────────────────────────────────

/// Ажил бүрийн task-д clone хийж өгнө (бүх талбар Arc эсвэл хямд clone).
#[derive(Clone)]
struct App {
    api: Arc<Api>,
    config: Arc<Config>,
    data_dir: PathBuf,
    log: LogFile,
    khost: String,
}

struct Guitsetgel {
    ur: Option<String>,
    zardal_usd: Option<f64>,
    zasvarlasan: Vec<String>,
    /// Ажил дуусах үеийн repo бүрийн HEAD — zasvarlasan хожим commit хийгдсэн
    /// эсэхийг `<hash>..HEAD`-ээс хайхад суурь болно (commit::repo_shalgakh)
    repo_head: Vec<(String, String)>,
}

/// Ажлын нэг удаагийн гүйцэтгэл хэрхэн төгссөн
enum Duusalt {
    /// Claude ажлаа хийж дууссан → /ur (duussan)
    Ur(Guitsetgel),
    /// Claude тодруулах асуулт гаргасан → /asuult (хэрэглэгч хариулахыг хүлээнэ)
    Asuult(usize),
}

/// Хэрэглэгчийн шийдэх зүйл гарвал Claude зогсоод асууж болох эсэх. Ажил дээр
/// унтраагаагүй бол (хуучин ажилд талбар байхгүй — анхдагч true) асууж болно.
fn asuukh_esekh(ajil: &Value) -> bool {
    ajil.get("asuultAsuukh").and_then(Value::as_bool) != Some(false)
}

/// Хэрэглэгчийн хариулсан (эсвэл алгассан) асуултууд — prompt-ийн «Тодруулга».
/// Хариулаагүй асуулт (`khariulsan` false) орохгүй.
fn khariulsan_asuultuud(ajil: &Value) -> Vec<claude::Asuult> {
    ajil.get("asuultuud")
        .cloned()
        .and_then(|v| serde_json::from_value::<Vec<claude::Asuult>>(v).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|a| a.khariulsan)
        .collect()
}

/// Commit хийгдээгүй ажлуудыг хэдэн секунд тутам шалгах
const COMMIT_SHALGAKH_SECS: u64 = 300;

impl App {
    fn new(config: Config, base_dir: &Path, log: LogFile) -> Self {
        let data_dir = base_dir.join("data");
        let _ = fs::create_dir_all(&data_dir);
        Self {
            api: Arc::new(Api::new(config.clone())),
            config: Arc::new(config),
            data_dir,
            log,
            khost: khost(),
        }
    }

    /// Өмнөх удаа энэ компьютер дээр ажиллаж байгаад service зогссон (khiigdej-д
    /// гацсан) ажлуудыг алдаатай гэж тэмдэглэнэ — цонхноос «Дахин ажиллуулах».
    async fn gatsan_tseverley(&self) {
        let jagsaalt = match self.api.get("/claudeAjil?tuluv=khiigdej").await {
            Ok(Value::Array(a)) => a,
            Ok(_) => return,
            Err(e) => {
                log_write(&self.log, "ERROR", &format!("Гацсан ажил шалгах: {e}"));
                return;
            }
        };
        for a in jagsaalt.iter().filter(|a| str_of(a, "khost") == self.khost) {
            let id = str_of(a, "_id");
            let body = json!({
                "tuluv": "aldaa",
                "aldaa": format!("{} дээрх service ажиллаж байхдаа зогссон", self.khost),
            });
            match self.api.post(&format!("/claudeAjil/{id}/ur"), body).await {
                Ok(_) => log_write(
                    &self.log,
                    "WARN",
                    &format!("Гацсан ажил алдаатай болголоо: {id}"),
                ),
                Err(e) => log_write(&self.log, "ERROR", &format!("{id}: {e}")),
            }
        }
    }

    /// Энэ компьютер дээр ажилласан, commit хийгдээгүй өөрчлөлттэй дууссан
    /// ажлуудыг шалгаж, хүн commit хийсэн (эсвэл өөрчлөлтийг хаясан) бол
    /// commit hash-ийг ажилд бичнэ. Service өөрөө commit/push хийхгүй — зөвхөн уншина.
    async fn commit_shalgay(&self) {
        let zam = format!("/claudeAjilCommitKhuleegdej?khost={}", self.khost);
        let jagsaalt = match self.api.get(&zam).await {
            Ok(Value::Array(a)) => a,
            Ok(_) => return,
            Err(e) => {
                log_write(&self.log, "ERROR", &format!("Commit шалгах жагсаалт: {e}"));
                return;
            }
        };
        'ajil: for a in &jagsaalt {
            let id = str_of(a, "_id");
            let duussan = str_of(a, "duussan");
            let mut repos: BTreeMap<String, Vec<String>> = BTreeMap::new();
            for m in a.get("zasvarlasan").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                if let Some((repo, file)) = commit::zasvar_zadlakh(m) {
                    repos.entry(repo).or_default().push(file);
                }
            }
            if repos.is_empty() {
                continue;
            }
            let suuriud: BTreeMap<&str, &str> = a
                .get("repoHead")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|h| (str_of(h, "repo"), str_of(h, "hash")))
                .collect();
            let mut dunguud = Vec::new();
            for (ner, failuud) in &repos {
                // Repo энэ компьютер дээр алга бол тодорхойлж чадахгүй — хүлээсээр
                let Some(rz) = repo_zam(&self.config, ner).filter(|z| Path::new(z).is_dir()) else {
                    continue 'ajil;
                };
                match commit::repo_shalgakh(ner, &rz, failuud, suuriud.get(ner.as_str()).copied(), duussan).await {
                    Ok(d) => dunguud.push(d),
                    Err(e) => {
                        log_write(&self.log, "WARN", &format!("{id}: {ner} commit шалгах: {e}"));
                        continue 'ajil;
                    }
                }
            }
            let Some((tuluv, commituud)) = commit::negtgekh(dunguud) else {
                continue;
            };
            let body = json!({ "duussan": duussan, "commitTuluv": tuluv, "commituud": commituud });
            match self.api.post(&format!("/claudeAjil/{id}/commit"), body).await {
                Ok(_) => log_write(
                    &self.log,
                    "INFO",
                    &format!(
                        "{id}: {tuluv} — {}",
                        commituud.iter().map(|c| format!("{} {}", c.repo, &c.hash[..c.hash.len().min(8)])).collect::<Vec<_>>().join(", ")
                    ),
                ),
                // Хооронд нь шинэ ээлж эхэлсэн — тэр ээлжийн дүнгээр дахин шалгана
                Err(ApiAldaa::Zurchil) => {}
                Err(e) => log_write(&self.log, "ERROR", &format!("{id} commit бичих: {e}")),
            }
        }
    }

    async fn guitsetgey(&self, ajil: Value) {
        let id = str_of(&ajil, "_id").to_string();
        log_write(
            &self.log,
            "INFO",
            &format!("Ажил эхэллээ: {id} «{}»", str_of(&ajil, "garchig")),
        );
        let (tuluv, g, aldaa) = match self.ajilluulya(&id, &ajil).await {
            Ok(Duusalt::Asuult(n)) => {
                log_write(
                    &self.log,
                    "INFO",
                    &format!("Ажил {id}: {n} тодруулах асуулт — хэрэглэгчийн хариултыг хүлээнэ"),
                );
                return;
            }
            Ok(Duusalt::Ur(g)) => ("duussan", g, None),
            Err((e, g)) => ("aldaa", g, Some(e)),
        };
        match &aldaa {
            None => log_write(
                &self.log,
                "INFO",
                &format!(
                    "Ажил дууслаа: {id} · зардал {} · өөрчлөлт {}",
                    g.zardal_usd
                        .map(|z| format!("${z:.2}"))
                        .unwrap_or_else(|| "-".into()),
                    g.zasvarlasan.len()
                ),
            ),
            Some(e) => log_write(&self.log, "ERROR", &format!("Ажил {id}: {e}")),
        }
        let body = json!({
            "tuluv": tuluv,
            "ur": g.ur,
            "aldaa": aldaa,
            "zardalUsd": g.zardal_usd,
            "zasvarlasan": g.zasvarlasan,
            "repoHead": g.repo_head.iter().map(|(repo, hash)| json!({ "repo": repo, "hash": hash })).collect::<Vec<_>>(),
        });
        if let Err(e) = self.api.post(&format!("/claudeAjil/{id}/ur"), body).await {
            log_write(
                &self.log,
                "ERROR",
                &format!("{id} үр дүн илгээхэд алдаа: {e}"),
            );
        }
    }

    /// Repo шалгах → зураг татах → Claude ажиллуулах. Алдаа гарсан ч тэр
    /// хүртэлх гүйцэтгэлийг (өөрчлөгдсөн файл) буцаана.
    async fn ajilluulya(&self, id: &str, ajil: &Value) -> Result<Duusalt, (String, Guitsetgel)> {
        let mut g = Guitsetgel {
            ur: None,
            zardal_usd: None,
            zasvarlasan: Vec::new(),
            repo_head: Vec::new(),
        };

        // Repo
        let nernuud = repo_nernuud(ajil);
        if nernuud.is_empty() {
            return Err(("Систем сонгоогүй (repo алга)".into(), g));
        }
        // Системийн зарим repo (package) энэ компьютер дээр байхгүй байж болно —
        // тэдгээрийг алгасч, үлдсэн дээр нь ажиллана. Нэг ч олдохгүй бол алдаа.
        let mut repos = Vec::new();
        let mut baikhgui = Vec::new();
        for ner in &nernuud {
            match repo_zam(&self.config, ner) {
                Some(zam) if Path::new(&zam).is_dir() => repos.push((ner.clone(), zam)),
                Some(zam) => baikhgui.push(format!("{ner} ({zam} хавтас алга)")),
                None => baikhgui.push(format!("{ner} (config.toml-д зам алга)")),
            }
        }
        if repos.is_empty() {
            return Err((
                format!("{}: repo олдсонгүй — {}", self.khost, baikhgui.join(", ")),
                g,
            ));
        }
        let baikhgui_mur = (!baikhgui.is_empty()).then(|| {
            format!(
                "⚠ {} дээр байхгүй тул алгаслаа: {}",
                self.khost,
                baikhgui.join(", ")
            )
        });
        if let Some(m) = &baikhgui_mur {
            log_write(&self.log, "WARN", &format!("{id}: {m}"));
        }
        // Нэмэлт ээлж бол өмнөх ээлжүүдэд Claude-ийн хийсэн (commit хийгдээгүй)
        // өөрчлөлт хүлээгдэж буй зүйл — түүн дээр үргэлжлүүлнэ. Түүнээс өөр
        // өөрчлөлт (хэрэглэгчийнх) байвал хуучнаараа bokhir гэж хамгаална.
        let (tuukh, umnukh_murnuud) = tuukh_unshikh(ajil);
        let mut bokhir = Vec::new();
        let mut umnukh_uurchlult = Vec::new();
        let mut tuluvuud = Vec::new();
        for (ner, zam) in &repos {
            match claude::git_status(zam).await {
                Ok(s) => {
                    let t = angilakh(&status_murnuud(ner, &s), &umnukh_murnuud);
                    match t {
                        RepoTuluv::Tseverkhen => {}
                        RepoTuluv::UmnukhEeljiin => umnukh_uurchlult.push(ner.clone()),
                        RepoTuluv::Bokhir => bokhir.push(ner.clone()),
                    }
                    tuluvuud.push(t);
                }
                Err(e) => return Err((format!("{ner}: git {e}"), g)),
            }
        }
        if !bokhir.is_empty() && !self.config.claude.bokhir_repo_zuvshuurukh {
            return Err((
                format!(
                    "Commit хийгдээгүй өөрчлөлттэй repo: {} — commit хийгээд «Дахин ажиллуулах» дарна уу",
                    bokhir.join(", ")
                ),
                g,
            ));
        }

        // git pull — Claude хамгийн сүүлийн код дээр ажиллана
        let mut pull_murnuud = Vec::new();
        if self.config.claude.git_pull {
            for ((ner, zam), tuluv) in repos.iter().zip(&tuluvuud) {
                let mur = match (claude::git_pull(zam).await, tuluv) {
                    (Ok(claude::PullUr::Tatsan(n)), _) => format!("⬇ {ner}: git pull — {n} шинэ commit татлаа"),
                    (Ok(claude::PullUr::Shine), _) => format!("✓ {ner}: git pull — аль хэдийн хамгийн сүүлийн хувилбар"),
                    (Ok(claude::PullUr::UpstreamGui), _) => {
                        format!("⚠ {ner}: upstream branch тохируулаагүй тул git pull алгаслаа")
                    }
                    // Цэвэр repo-г шинэчилж чадаагүй бол хуучирсан код дээр
                    // засвар хийж дараа нь merge зөрчил үүсгэхээс татгалзана.
                    (Err(e), RepoTuluv::Tseverkhen) => {
                        return Err((
                            format!(
                                "{ner}: git pull амжилтгүй — хуучирсан код дээр ажиллахгүй. \
                                 Repo-г гараар шинэчлээд «Дахин ажиллуулах» дарна уу: {}",
                                claude::tovch(&e, 300)
                            ),
                            g,
                        ))
                    }
                    // Commit хийгдээгүй өөрчлөлттэй (өмнөх ээлж эсвэл зөвшөөрсөн
                    // bokhir) repo: git ff-only зөрчилтэй бол юу ч өөрчлөхгүй
                    // зогсдог тул одоогийн төлөв дээрээ үргэлжлүүлнэ.
                    (Err(e), _) => format!(
                        "⚠ {ner}: commit хийгдээгүй өөрчлөлттэй тул git pull хийж чадсангүй, одоогийн код дээр үргэлжлүүлнэ: {}",
                        claude::tovch(&e, 300)
                    ),
                };
                log_write(&self.log, "INFO", &format!("{id}: {mur}"));
                pull_murnuud.push(mur);
            }
        }

        // Зураг
        let ajil_dir = self.data_dir.join(id);
        if let Err(e) = fs::create_dir_all(&ajil_dir) {
            return Err((format!("{}: {e}", ajil_dir.display()), g));
        }
        let mut zurguud = Vec::new();
        for (i, z) in ajil
            .get("zurguud")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
        {
            let zid = str_of(z, "id");
            if zid.is_empty() {
                continue;
            }
            let (bytes, ext) = match self.api.zurag_tatay(zid).await {
                Ok(r) => r,
                Err(e) => return Err((e, g)),
            };
            let zam = ajil_dir.join(format!("zurag-{}.{ext}", i + 1));
            if let Err(e) = fs::write(&zam, bytes) {
                return Err((format!("{}: {e}", zam.display()), g));
            }
            zurguud.push((
                zam.display().to_string().replace('\\', "/"),
                str_of(z, "tailbar").to_string(),
            ));
        }

        let shalgalt_dir = match claude::shalgalt_beltgey(&ajil_dir) {
            Ok(z) => z,
            Err(e) => return Err((e, g)),
        };

        let zamuud: Vec<String> = repos.iter().map(|(_, z)| z.clone()).collect();
        // Алгассан repo, git pull-ийн дүнг цонхны явцад эхэнд нь харуулна
        let buf: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(
            baikhgui_mur.into_iter().chain(pull_murnuud).collect(),
        ));

        // Claude эхлэхээс өмнөх commit хийгдээгүй өөрчлөлтийн тамга — ажлын
        // дараа хэрэглэгчийн урьдчилан байсан өөрчлөлтийг zasvarlasan-аас ялгана
        let mut umnu_tamga = Vec::with_capacity(repos.len());
        for (ner, zam) in &repos {
            let t = match claude::git_status(zam).await {
                Ok(s) => commit::tamga_avakh(zam, &status_murnuud(ner, &s)).await,
                Err(e) => Err(e),
            };
            match t {
                Ok(t) => umnu_tamga.push(t),
                Err(e) => return Err((format!("{ner}: git {e}"), g)),
            }
        }

        // Claude ажлыг шууд гүйцэтгэнэ; хэрэглэгчийн шийдэх зүйлтэй тулгарвал
        // зогсоод асуумж буцаана (доор /asuult). Хариулсан асуултууд нь prompt-ийн
        // «Тодруулга» хэсэгт орно.
        let asuukh = asuukh_esekh(ajil);
        let khariulsan = khariulsan_asuultuud(ajil);
        let prompt = claude::prompt_beltgey(
            &claude::Ajil {
                garchig: str_of(ajil, "garchig"),
                tailbar: str_of(ajil, "tailbar"),
                system: str_of(ajil, "system"),
                repos: &repos,
                baikhgui: &baikhgui,
                zurguud: &zurguud,
                nemelt_zaavar: str_of(ajil, "nemeltZaavar"),
                tuukh: &tuukh,
                umnukh_uurchlult: &umnukh_uurchlult,
                shalgalt_dir: &shalgalt_dir,
                asuukh,
                oilgolt: str_of(ajil, "oilgolt"),
                asuultuud: &khariulsan,
            },
            &bokhir,
        );
        let _ = fs::write(ajil_dir.join("prompt.md"), &prompt);

        let ur = self
            .claude_khyanaltai(
                id,
                &buf,
                asuukh,
                &prompt,
                &zamuud,
                &ajil_dir,
                &ajil_dir.join("claude.jsonl"),
            )
            .await
            .and_then(|u| {
                let butets = u
                    .butets
                    .ok_or("Claude JSON хариу (structured_output) буцаасангүй")?;
                Ok((claude::ajliin_ur_tseverley(&butets)?, u.zardal_usd))
            });
        self.yavts_ilgeey(id, &buf).await;

        for ((ner, zam), umnu) in repos.iter().zip(&umnu_tamga) {
            if let Ok(s) = claude::git_status(zam).await {
                // Дараагийн ээлжид angilakh эдгээрийг ижил хэлбэрээр харьцуулна.
                // Зөвхөн Claude-ийн өөрчлөлт — хэрэглэгчийн урьдчилан байсан
                // өөрчлөлтийг оруулбал commit_shalgay ажлыг хэзээ ч «орсон» гэж тэмдэглэхгүй.
                let murnuud = status_murnuud(ner, &s);
                match commit::tamga_avakh(zam, &murnuud).await {
                    Ok(daraa) => g.zasvarlasan.extend(commit::claude_zasvar(umnu, &daraa, &umnukh_murnuud)),
                    Err(e) => {
                        // Ялгаж чадаагүй бол Claude-ийн өөрчлөлтийг алдахаас сэргийлж бүгдийг бичнэ
                        log_write(&self.log, "WARN", &format!("{id}: {ner} өөрчлөлт ялгах: {e} — бүх мөрийг бичлээ"));
                        g.zasvarlasan.extend(murnuud);
                    }
                }
            }
            if let Ok(h) = claude::git(zam, &["rev-parse", "HEAD"], Duration::from_secs(30)).await {
                g.repo_head.push((ner.clone(), h.trim().to_string()));
            }
        }

        let (ajliin_ur, zardal_usd) = match ur {
            Ok(r) => r,
            Err(e) => return Err((e, g)),
        };

        // Хэрэглэгчийн шийдэх зүйлтэй тулгарсан — асуумжийг цонх руу илгээж
        // хариултыг хүлээнэ. Энэ удаагийн (хагас) өөрчлөлтийг zasvarlasan-д
        // хадгална: хариулсны дараах ээлж үүнийг Claude-ийнх гэж таньж үргэлжлүүлнэ.
        if asuukh && !ajliin_ur.asuultuud.is_empty() {
            let n = ajliin_ur.asuultuud.len();
            if let Ok(mut b) = buf.lock() {
                b.push(format!("❓ Claude {n} асуулт тавьлаа — таны хариултыг хүлээж байна"));
            }
            // /asuult нь khiigdej-ээс гарах тул явцыг өмнө нь илгээнэ
            self.yavts_ilgeey(id, &buf).await;
            let body = json!({
                "oilgolt": ajliin_ur.tailan,
                "asuultuud": ajliin_ur.asuultuud,
                "zardalUsd": zardal_usd,
                "zasvarlasan": g.zasvarlasan,
                "repoHead": g.repo_head.iter().map(|(repo, hash)| json!({ "repo": repo, "hash": hash })).collect::<Vec<_>>(),
            });
            return match self.api.post(&format!("/claudeAjil/{id}/asuult"), body).await {
                Ok(_) => Ok(Duusalt::Asuult(n)),
                Err(ApiAldaa::Zurchil) => Err(("Цонхноос цуцалсан".into(), g)),
                Err(e) => Err((format!("Асуулт илгээж чадсангүй: {e}"), g)),
            };
        }

        // avtomat_push repo-д амжилттай ажлын өөрчлөлтийг service commit + push хийнэ.
        // repo_head-ийг commit-оос ӨМНӨ авсан тул commit_shalgay энэ commit-ийг
        // `<суурь>..HEAD`-ээс олж ажлыг «шинэчлэлтэд орсон» болгоно.
        let mut push_murnuud = Vec::new();
        for (ner, zam) in &repos {
            if !push::idevkhtei(&self.config.claude.avtomat_push, ner) {
                continue;
            }
            let failuud = push::repo_failuud(&g.zasvarlasan, ner);
            if failuud.is_empty() {
                continue;
            }
            let mur = match push::commit_push(ner, zam, &failuud, str_of(ajil, "garchig"), id).await {
                Ok(Some(m)) => m,
                Ok(None) => continue,
                Err(e) => format!("⚠ {ner}: автомат commit/push — {e}"),
            };
            log_write(&self.log, "INFO", &format!("{id}: {mur}"));
            push_murnuud.push(mur);
        }
        if !push_murnuud.is_empty() {
            if let Ok(mut b) = buf.lock() {
                b.extend(push_murnuud.iter().cloned());
            }
            self.yavts_ilgeey(id, &buf).await;
        }
        g.ur = Some(if push_murnuud.is_empty() {
            ajliin_ur.tailan
        } else {
            format!("{}\n\n---\n{}", ajliin_ur.tailan, push_murnuud.join("\n"))
        });
        g.zardal_usd = zardal_usd;
        Ok(Duusalt::Ur(g))
    }


    /// Claude-г ажиллуулна — явцыг YAVTS_SECS тутам backend руу илгээж,
    /// цонхноос цуцалсан бол зогсооно.
    #[allow(clippy::too_many_arguments)]
    async fn claude_khyanaltai(
        &self,
        id: &str,
        buf: &Arc<Mutex<Vec<String>>>,
        asuukh: bool,
        prompt: &str,
        zamuud: &[String],
        ajil_dir: &Path,
        stream_path: &Path,
    ) -> Result<claude::ClaudeUr, String> {
        let log = self.log.clone();
        let buf2 = buf.clone();
        let yavts = move |m: &str| {
            log_write(&log, "CLAUDE", m);
            if let Ok(mut b) = buf2.lock() {
                b.push(m.to_string());
            }
        };
        let claude_fut = claude::ajilluulya(
            &self.config.claude,
            asuukh,
            prompt,
            zamuud,
            ajil_dir,
            stream_path,
            yavts,
        );
        tokio::pin!(claude_fut);
        let mut interval = tokio::time::interval(Duration::from_secs(YAVTS_SECS));
        interval.tick().await;
        loop {
            tokio::select! {
                r = &mut claude_fut => break r,
                _ = interval.tick() => {
                    if self.yavts_ilgeey(id, buf).await.as_deref() == Some("tsutsalsan") {
                        // claude_fut drop болоход процесс kill_on_drop-оор зогсоно
                        break Err("Цонхноос цуцалсан".into());
                    }
                }
            }
        }
    }

    /// Хуримтлагдсан явцын мөрийг илгээж, ажлын одоогийн төлөвийг буцаана.
    async fn yavts_ilgeey(&self, id: &str, buf: &Arc<Mutex<Vec<String>>>) -> Option<String> {
        let murnuud: Vec<String> = buf
            .lock()
            .map(|mut b| std::mem::take(&mut *b))
            .unwrap_or_default();
        match self
            .api
            .post(
                &format!("/claudeAjil/{id}/yavts"),
                json!({ "murnuud": murnuud }),
            )
            .await
        {
            Ok(v) => v.get("tuluv").and_then(Value::as_str).map(str::to_string),
            Err(e) => {
                log_write(&self.log, "WARN", &format!("Явц илгээхэд алдаа: {e}"));
                None
            }
        }
    }
}

/// shine ажлуудыг zereg_max хүртэл зэрэг ажиллуулна. Repo нь давхцсан ажлууд
/// зэрэг ажиллахгүй — shine төлөвтөө (цонхонд «Хүлээгдэж буй») үлдэж, repo нь
/// чөлөөлөгдсөний дараа илгээсэн дарааллаараа эхэлнэ.
struct Tuluvlugch {
    app: App,
    /// Ажиллаж буй ажлуудын түгжсэн repo-ууд (repo_tulkhuur)
    ajillaj: Ajillaj,
    tasks: tokio::task::JoinSet<()>,
}

impl Tuluvlugch {
    fn new(app: App) -> Self {
        Self {
            app,
            ajillaj: Arc::default(),
            tasks: tokio::task::JoinSet::new(),
        }
    }

    /// Дууссан task-уудыг цэвэрлэнэ. Panic гарсан бол log-д бичнэ (RepoTugjee
    /// drop-оор repo нь чөлөөлөгдсөн; ажил khiigdej-д үлдвэл дараагийн эхлэлд
    /// gatsan_tseverley алдаатай болгоно).
    fn duussan_tseverley(&mut self) {
        while let Some(r) = self.tasks.try_join_next() {
            self.task_ur(r);
        }
    }

    fn task_ur(&self, r: Result<(), tokio::task::JoinError>) {
        if let Err(e) = r {
            if e.is_panic() {
                log_write(&self.app.log, "ERROR", &format!("Ажлын task panic: {e}"));
            }
        }
    }

    async fn shalgay(&mut self) {
        self.duussan_tseverley();
        let sul = self
            .app
            .config
            .zereg_max
            .max(1)
            .saturating_sub(self.tasks.len());
        if sul == 0 {
            return;
        }
        let jagsaalt = match self.app.api.get("/claudeAjil?tuluv=shine").await {
            Ok(Value::Array(a)) => a,
            Ok(_) => {
                log_write(&self.app.log, "ERROR", "/claudeAjil: массив биш хариу");
                return;
            }
            Err(e) => {
                log_write(&self.app.log, "ERROR", &format!("/claudeAjil: {e}"));
                return;
            }
        };
        let repos: Vec<Vec<String>> = jagsaalt
            .iter()
            .map(|a| lokal_repo_tulkhuuruud(&self.app.config, a))
            .collect();
        let ajillaj = self
            .ajillaj
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        for i in songokh(&repos, &ajillaj, sul) {
            let id = str_of(&jagsaalt[i], "_id").to_string();
            let body = json!({ "khost": self.app.khost });
            match self
                .app
                .api
                .post(&format!("/claudeAjil/{id}/avya"), body)
                .await
            {
                Ok(ajil) => {
                    // Түгжээг /avya-ийн хариунаас (хамгийн сүүлийн system) тооцно
                    let tugjikh = lokal_repo_tulkhuuruud(&self.app.config, &ajil);
                    let app = self.app.clone();
                    let ajillaj = self.ajillaj.clone();
                    self.tasks.spawn(async move {
                        let _tugjee = RepoTugjee::avya(ajillaj, tugjikh, &app.log, &id).await;
                        app.guitsetgey(ajil).await;
                    });
                }
                // Өөр service авсан, эсвэл хооронд нь ноороглосон/цуцалсан
                Err(ApiAldaa::Zurchil) => continue,
                Err(e) => {
                    log_write(&self.app.log, "ERROR", &format!("{id} авахад алдаа: {e}"));
                    return;
                }
            }
        }
    }
}

async fn run_service_loop(
    config: Config,
    base_dir: PathBuf,
    log: LogFile,
    mut shutdown_rx: oneshot::Receiver<()>,
) {
    let app = App::new(config, &base_dir, log.clone());
    log_write(
        &log,
        "INFO",
        &format!(
            "rentlyClaudeNote эхэллээ: {} ({} сек тутам, зэрэг {}), khost {}, repo_undsen: {}, repo: {}",
            app.config.url,
            app.config.shalgakh_secs,
            app.config.zereg_max.max(1),
            app.khost,
            if app.config.repo_undsen.is_empty() { "-" } else { &app.config.repo_undsen },
            app.config.repos.keys().cloned().collect::<Vec<_>>().join(", ")
        ),
    );
    app.gatsan_tseverley().await;
    let shalgakh = Duration::from_secs(app.config.shalgakh_secs.max(10));
    let mut t = Tuluvlugch::new(app);
    let mut commit_suuld: Option<std::time::Instant> = None;
    loop {
        tokio::select! {
            _ = &mut shutdown_rx => break,
            _ = t.shalgay() => {}
        }
        if commit_suuld.map_or(true, |s| s.elapsed() >= Duration::from_secs(COMMIT_SHALGAKH_SECS)) {
            commit_suuld = Some(std::time::Instant::now());
            tokio::select! {
                _ = &mut shutdown_rx => break,
                _ = t.app.commit_shalgay() => {}
            }
        }
        // Ажил дуусмагц дараалалд хүлээж буйг шууд эхлүүлнэ, эс бөгөөс shalgakh_secs-ийн дараа
        tokio::select! {
            _ = &mut shutdown_rx => break,
            _ = sleep(shalgakh) => {}
            Some(r) = t.tasks.join_next(), if !t.tasks.is_empty() => t.task_ur(r),
        }
    }
    // Зогсоох үед ажиллаж буй Claude-уудыг зогсооно (abort → kill_on_drop); ажлууд
    // khiigdej-д үлдэж, дараагийн эхлэлд gatsan_tseverley алдаатай болгоно.
    t.tasks.shutdown().await;
    log_write(&log, "INFO", "rentlyClaudeNote зогслоо");
}

/// `rentlyClaudeNote.exe once` — хүлээгдэж буй бүх ажлыг (zereg_max хүртэл зэрэг)
/// гүйцэтгэж дуусаад гарна.
async fn run_once(config: Config, base_dir: PathBuf, log: LogFile) {
    let app = App::new(config, &base_dir, log);
    app.gatsan_tseverley().await;
    let mut t = Tuluvlugch::new(app);
    loop {
        t.shalgay().await;
        match t.tasks.join_next().await {
            Some(r) => t.task_ur(r),
            None => break,
        }
    }
}

fn console_run(
    f: impl FnOnce(Config, PathBuf, LogFile) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()>>>,
) {
    let (config, base_dir) = load_config();
    let log = open_log(&base_dir);
    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(f(config, base_dir, log));
}

fn console_loop() {
    console_run(|c, d, l| {
        let (_tx, rx) = oneshot::channel::<()>();
        Box::pin(async move {
            let _tx = _tx;
            run_service_loop(c, d, l, rx).await
        })
    });
}

// ─── Windows Service ──────────────────────────────────────────────────────────

define_windows_service!(ffi_service_main, service_main);

fn service_main(_args: Vec<OsString>) {
    if let Err(e) = run_as_service() {
        let log = open_log(&get_exe_dir());
        log_write(&log, "ERROR", &format!("Service алдаатай зогслоо: {:?}", e));
    }
}

fn run_as_service() -> windows_service::Result<()> {
    let (config, base_dir) = load_config();
    let log = open_log(&base_dir);

    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    let shutdown_tx = Arc::new(Mutex::new(Some(shutdown_tx)));

    let status_handle = service_control_handler::register(SERVICE_NAME, {
        let tx = Arc::clone(&shutdown_tx);
        move |event| match event {
            ServiceControl::Stop | ServiceControl::Shutdown => {
                if let Ok(mut guard) = tx.lock() {
                    if let Some(sender) = guard.take() {
                        let _ = sender.send(());
                    }
                }
                ServiceControlHandlerResult::NoError
            }
            ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
            _ => ServiceControlHandlerResult::NotImplemented,
        }
    })?;

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Running,
        controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    tokio::runtime::Runtime::new()
        .unwrap()
        .block_on(run_service_loop(config, base_dir, log.clone(), shutdown_rx));

    status_handle.set_service_status(ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: ServiceState::Stopped,
        controls_accepted: ServiceControlAccept::empty(),
        exit_code: ServiceExitCode::Win32(0),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    })?;

    Ok(())
}

// ─── Service install / uninstall ─────────────────────────────────────────────

fn install_service() -> windows_service::Result<()> {
    let manager =
        ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CREATE_SERVICE)?;
    let exe_path = std::env::current_exe().unwrap();

    let _svc = manager.create_service(
        &ServiceInfo {
            name: OsString::from(SERVICE_NAME),
            display_name: OsString::from(SERVICE_DISPLAY_NAME),
            service_type: ServiceType::OWN_PROCESS,
            start_type: ServiceStartType::AutoStart,
            error_control: ServiceErrorControl::Normal,
            executable_path: exe_path,
            launch_arguments: vec![],
            dependencies: vec![],
            account_name: None, // LocalSystem
            account_password: None,
        },
        ServiceAccess::empty(),
    )?;

    println!("✓ Service суулгагдлаа: {}", SERVICE_NAME);
    println!("  Claude-ийн нэвтрэлттэй Windows хэрэглэгчээр ажиллуулах (README-г үз):");
    println!(
        "  sc config {} obj= \".\\<хэрэглэгч>\" password= \"<нууц үг>\"",
        SERVICE_NAME
    );
    println!("  Эхлүүлэх: sc start {}", SERVICE_NAME);
    Ok(())
}

fn uninstall_service() -> windows_service::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)?;
    let svc = manager.open_service(SERVICE_NAME, ServiceAccess::DELETE | ServiceAccess::STOP)?;
    svc.delete()?;
    println!("✓ Service устгагдлаа: {}", SERVICE_NAME);
    Ok(())
}

// ─── Main ─────────────────────────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("install") => {
            install_service().unwrap_or_else(|e| eprintln!("Суулгахад алдаа: {:?}", e));
        }
        Some("uninstall") => {
            uninstall_service().unwrap_or_else(|e| eprintln!("Устгахад алдаа: {:?}", e));
        }
        // Console горим — гараар туршихад
        Some("run") => console_loop(),
        Some("once") => console_run(|c, d, l| Box::pin(run_once(c, d, l))),
        Some("help") | Some("--help") | Some("-h") => {
            println!(
                "Ашиглалт:\n  rentlyClaudeNote.exe run        - console горимд ажиллуулна\n  rentlyClaudeNote.exe once       - нэг удаа шалгаж хүлээгдэж буй ажлуудыг гүйцэтгэнэ\n  rentlyClaudeNote.exe install    - Windows Service болгож бүртгэнэ (Administrator)\n  rentlyClaudeNote.exe uninstall  - Windows Service-г устгана (Administrator)"
            );
        }
        _ => {
            // SCM-ээс дуудагдах үед — service dispatcher эхлүүлнэ
            service_dispatcher::start(SERVICE_NAME, ffi_service_main).unwrap_or_else(|_| {
                // SCM байхгүй (гараар ажиллуулсан) бол console горимд орно
                console_loop();
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_repos_ba_claude_unshina() {
        let c: Config = toml::from_str(
            r#"
            nevtrekhNer = "CAdmin1"
            [repos]
            tureesBack = "D:/Work/tureesBack"
            parking-v2 = "D:/Work/parking-v2"
            [claude]
            budget_usd = 2.5
            "#,
        )
        .unwrap();
        assert_eq!(
            c.repos.get("parking-v2").map(String::as_str),
            Some("D:/Work/parking-v2")
        );
        assert_eq!(c.claude.budget_usd, 2.5);
        assert_eq!(c.claude.timeout_min, 60);
        assert!(!c.claude.bokhir_repo_zuvshuurukh);
        // [claude] хэсэгтэй ч git_pull-ийг дурдаагүй бол анхдагчаар асаалттай
        assert!(c.claude.git_pull);
        // Дурдаагүй бол автомат push хэнд ч хийхгүй
        assert!(c.claude.avtomat_push.is_empty());
        assert_eq!(c.shalgakh_secs, 60);

        let c: Config = toml::from_str("[claude]\navtomat_push = [\"irtsBack\", \"irtsWeb\"]").unwrap();
        assert!(push::idevkhtei(&c.claude.avtomat_push, "irtsWeb"));
        assert!(!push::idevkhtei(&c.claude.avtomat_push, "tureesBack"));
    }

    #[test]
    fn claude_khesggui_config() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.claude.exe, "claude");
        assert!(c.repos.is_empty());
    }

    #[test]
    fn repo_zam_tokhirgoo_ba_undsen() {
        let c: Config = toml::from_str(
            r#"
            repo_undsen = "D:/Work/"
            [repos]
            turees = "E:/busad/turees"
            "#,
        )
        .unwrap();
        assert_eq!(repo_zam(&c, "turees").as_deref(), Some("E:/busad/turees"));
        assert_eq!(repo_zam(&c, "sukhWeb").as_deref(), Some("D:/Work/sukhWeb"));
        assert_eq!(repo_zam(&c, "../Windows"), None);
        assert_eq!(repo_zam(&c, "a/b"), None);
        assert_eq!(repo_zam(&c, ""), None);

        let khooson: Config = toml::from_str("").unwrap();
        assert_eq!(repo_zam(&khooson, "turees"), None);
    }

    #[test]
    fn umnukh_eeljiin_uurchlultiig_tanina() {
        let ajil = json!({
            "nemeltZaavar": "дахин",
            "tuukh": [
                { "ur": "1", "zasvarlasan": ["parking-v2: M src/serivice/sdkService.ts"] },
                // Эрт алдаа гарсан ээлж — zasvarlasan хоосон
                { "zaavar": "2", "aldaa": "bokhir", "zasvarlasan": [] }
            ]
        });
        let (tuukh, umnukh) = tuukh_unshikh(&ajil);
        assert_eq!(tuukh.len(), 2);
        assert_eq!(tuukh[1].zaavar, "2");
        assert_eq!(tuukh[1].aldaa, "bokhir");

        let claude_iin = status_murnuud("parking-v2", " M src/serivice/sdkService.ts\n");
        assert_eq!(angilakh(&claude_iin, &umnukh), RepoTuluv::UmnukhEeljiin);
        // Хэрэглэгчийн өөр файл нэмэгдсэн бол хамгаална
        let kholimog = status_murnuud("parking-v2", " M src/serivice/sdkService.ts\n?? tmp.txt\n");
        assert_eq!(angilakh(&kholimog, &umnukh), RepoTuluv::Bokhir);
        // Өөр repo-д ижил замтай файл — repo нэрээр ялгана
        assert_eq!(
            angilakh(
                &status_murnuud("turees", " M src/serivice/sdkService.ts"),
                &umnukh
            ),
            RepoTuluv::Bokhir
        );
        assert_eq!(
            angilakh(&status_murnuud("turees", ""), &umnukh),
            RepoTuluv::Tseverkhen
        );
        // Анхны ээлж (tuukh-гүй) — ямар ч өөрчлөлт bokhir
        let (_, khooson) = tuukh_unshikh(&json!({}));
        assert_eq!(angilakh(&claude_iin, &khooson), RepoTuluv::Bokhir);
    }

    fn r(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn songokh_davkhtsakhgui_ajluudyg_zereg_ekhluulne() {
        // PosEase, ParkEase — давхцахгүй тул зэрэг; Rently нь ParkEase-тэй давхцана
        let jagsaalt = vec![
            r(&["posweb", "posbackv2"]),
            r(&["turees", "tureesback"]),
            r(&["turees", "tureesback"]),
        ];
        assert_eq!(songokh(&jagsaalt, &HashSet::new(), 3), vec![0, 1]);
        // zereg_max хязгаар
        assert_eq!(songokh(&jagsaalt, &HashSet::new(), 1), vec![0]);
    }

    #[test]
    fn songokh_ajillaj_buig_khuleene_daraallaa_khadgalna() {
        let ajillaj: HashSet<String> = r(&["turees"]).into_iter().collect();
        let jagsaalt = vec![
            r(&["turees", "parking-v2"]), // ажиллаж буйтай давхцана → хүлээнэ
            r(&["parking-v2"]),           // хүлээж буй урдахтай давхцана → урдуур орохгүй
            r(&["posweb"]),               // чөлөөтэй
        ];
        assert_eq!(songokh(&jagsaalt, &ajillaj, 3), vec![2]);
        // "Бүх систем" бүх repo-г түгжинэ — дараагийнх бүгд хүлээнэ
        let bukh = vec![
            r(&["turees", "posweb", "zevtabs"]),
            r(&["posweb"]),
            r(&["sukhweb"]),
        ];
        assert_eq!(songokh(&bukh, &HashSet::new(), 3), vec![0, 2]);
    }

    #[test]
    fn repo_tulkhuur_windows_zam() {
        assert_eq!(
            repo_tulkhuur("D:\\Work\\Turees\\"),
            repo_tulkhuur("d:/work/turees")
        );
    }

    #[test]
    fn repo_tugjee_drop_khiikhed_chuluulnu() {
        let ajillaj: Ajillaj = Arc::default();
        let log: LogFile = Arc::new(Mutex::new(Box::new(std::io::sink())));
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap();
        rt.block_on(async {
            let t = RepoTugjee::avya(ajillaj.clone(), r(&["turees"]), &log, "x").await;
            assert!(ajillaj.lock().unwrap().contains("turees"));
            drop(t);
            assert!(ajillaj.lock().unwrap().is_empty());
        });
    }

    #[test]
    fn zereg_max_default_1() {
        let c: Config = toml::from_str("").unwrap();
        assert_eq!(c.zereg_max, 1);
        let c: Config = toml::from_str("zereg_max = 3").unwrap();
        assert_eq!(c.zereg_max, 3);
    }

    #[test]
    fn config_jishee_unshigdana() {
        let c: Config = toml::from_str(include_str!("../config.example.toml")).unwrap();
        assert_eq!(c.claude.model, "claude-opus-5-5");
    }

    #[test]
    fn asuukh_esekh_anhdagch_true() {
        // Шинэ болон хуучин (талбаргүй) ажил — шаардлагатай бол асууж болно
        assert!(asuukh_esekh(&json!({})));
        assert!(asuukh_esekh(&json!({ "asuultAsuukh": true })));
        assert!(!asuukh_esekh(&json!({ "asuultAsuukh": false })));
    }

    #[test]
    fn khariulsan_asuultuud_unshina() {
        let asuult = |a: &str, khariulsan: bool| {
            json!({
                "_id": "x",
                "asuult": a,
                "tovch": "Хуваах",
                "olonSongolt": false,
                "songoltuud": [{ "ner": "Тэнцүү", "tailbar": "", "sanal": true }, { "ner": "Талбайгаар" }],
                "khariu": if khariulsan { json!(["Талбайгаар"]) } else { json!([]) },
                "busad": "",
                "khariulsan": khariulsan
            })
        };
        let a = khariulsan_asuultuud(&json!({ "asuultuud": [asuult("1?", true), asuult("2?", false)] }));
        // Хариулаагүй асуулт prompt-д орохгүй
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].asuult, "1?");
        assert_eq!(a[0].khariu, vec!["Талбайгаар".to_string()]);
        assert!(a[0].songoltuud[0].sanal && !a[0].songoltuud[1].sanal);
        assert!(khariulsan_asuultuud(&json!({})).is_empty());
    }

    #[test]
    fn asuult_tavisan_uurchlultiig_claude_iinkh_gej_tanina() {
        // Claude асуулт тавихдаа хийсэн өөрчлөлт ажлын zasvarlasan-д хадгалагдана —
        // хариулсны дараах ажиллагаанд bokhir биш, өмнөх ээлжийнх гэж үзнэ
        let (_, umnukh) = tuukh_unshikh(&json!({ "zasvarlasan": ["turees: M src/a.js"] }));
        let murnuud = status_murnuud("turees", " M src/a.js\n");
        assert_eq!(angilakh(&murnuud, &umnukh), RepoTuluv::UmnukhEeljiin);
    }

    #[test]
    fn zurgiin_ext() {
        assert_eq!(zurag_ext("image/jpeg"), "jpg");
        assert_eq!(zurag_ext("image/png; charset=binary"), "png");
        assert_eq!(zurag_ext(""), "png");
    }
}
