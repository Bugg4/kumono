use crate::{ cli::ARGS, http::CLIENT, progress::{ DownloadState, n_fmt }, target::Target };
use anyhow::{ Context, Result, bail };
use futures_util::StreamExt;
use indicatif::{ ProgressBar, ProgressStyle };
use reqwest::StatusCode;
use serde::Deserialize;
use std::{ collections::HashSet, error::Error, fmt, io::SeekFrom, path::PathBuf, thread };
use tokio::{
    fs::{ self, File },
    io::{ AsyncSeekExt, AsyncWriteExt },
    sync::mpsc,
    time::{ Duration, sleep },
};

const API_DELAY: Duration = Duration::from_millis(100);

#[derive(Default)]
pub struct Profile {
    pub target: Target,
    post_count: usize,
    posts: Vec<Post>,
    pub files: HashSet<PostFile>,
}

impl fmt::Display for Profile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(post) = &self.target.post {
            let files = match self.files.len() {
                0 => "no files",
                1 => "1 file",
                n => &format!("{} files", n_fmt(n as u64)),
            };

            write!(f, "{}/{}/{post} has {files}", self.target.service, self.target.user)
        } else if let Some(page) = &self.target.page {
            let posts = match self.post_count {
                0 => "no posts",
                1 => "1 post",
                n => &(n_fmt(n as u64) + " posts"),
            };

            let files = match self.files.len() {
                0 => ", but no files",
                1 => ", containing 1 file",
                n => &format!(", containing {} files", n_fmt(n as u64)),
            };

            write!(
                f,
                "{}/{} page {} has {posts}{files}",
                self.target.service,
                self.target.user,
                (page.parse::<usize>().unwrap() + 50) / 50
            )
        } else {
            let posts = match self.post_count {
                0 => "no posts",
                1 => "1 post",
                n => &(n_fmt(n as u64) + " posts"),
            };

            let files = if self.post_count == 0 {
                ""
            } else {
                match self.files.len() {
                    0 => ", but no files",
                    1 => ", containing 1 file",
                    n => &format!(", containing {} files", n_fmt(n as u64)),
                }
            };

            write!(f, "{}/{} has {posts}{files}", self.target.service, self.target.user)
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct Channel {
    id: String, // "455285536341491716"
    // name: String, // "news"
}

fn page_progress(mut msg_rx: mpsc::UnboundedReceiver<String>) {
    let bar = ProgressBar::new_spinner();

    bar.set_style(ProgressStyle::with_template("[{elapsed_precise}] {msg}").unwrap());

    bar.enable_steady_tick(Duration::from_millis(200));

    while let Some(msg) = msg_rx.blocking_recv() {
        bar.set_message(msg);
    }

    bar.finish();
}

impl Profile {
    pub async fn new(target: &Target) -> Result<Self> {
        let mut profile = Self { target: target.clone(), ..Default::default() };

        if profile.target.service == "discord" {
            profile.init_posts_discord().await?;
        } else {
            profile.target.exists().await?;
            profile.init_posts_standard().await?;
        }

        profile.init_files();

        eprintln!("{profile}");

        profile.posts.clear();

        Ok(profile)
    }

    async fn init_posts_discord(&mut self) -> Result<()> {
        let channels: Vec<Channel> = if let Some(channel) = &self.target.post {
            vec![Channel {
                id: channel.to_string(),
            }]
        } else {
            CLIENT.get(
                format!("https://kemono.su/api/v1/discord/channel/lookup/{}", self.target.user)
            )
                .send().await?
                .json().await?
        };

        let (msg_tx, msg_rx) = mpsc::unbounded_channel::<String>();

        thread::spawn(move || page_progress(msg_rx));

        for channel in channels {
            let mut offset = 0;

            loop {
                msg_tx.send(
                    format!(
                        "Retrieving posts for discord/{}/{} page #{}",
                        self.target.user,
                        channel.id,
                        (offset + 150) / 150
                    )
                )?;

                let mut posts: Vec<Post>;

                let url = format!(
                    "https://kemono.su/api/v1/discord/channel/{}?o={offset}",
                    channel.id
                );

                loop {
                    sleep(API_DELAY).await;

                    let response = CLIENT.get(&url).send().await?;

                    match response.status() {
                        StatusCode::OK => {
                            posts = response.json().await?;
                            break;
                        }

                        StatusCode::TOO_MANY_REQUESTS => {
                            sleep(ARGS.rate_limit_backoff).await;
                        }

                        status => bail!("{url} returned unexpected status: {status}"),
                    }
                }

                if posts.is_empty() {
                    break;
                }

                self.posts.append(&mut posts);

                offset += 150;
            }
        }

        Ok(())
    }

    async fn init_posts_standard(&mut self) -> Result<()> {
        if let Some(post) = &self.target.post {
            let post: Post = CLIENT.get(
                format!(
                    "https://{}.su/api/v1/{}/user/{}/post/{post}",
                    self.target.site(),
                    self.target.service,
                    self.target.user
                )
            )
                .send().await?
                .json().await?;

            self.posts.push(post);
        } else {
            let mut offset = if let Some(page) = self.target.page.as_ref() {
                page.parse()?
            } else {
                0
            };

            let (msg_tx, msg_rx) = mpsc::unbounded_channel::<String>();

            thread::spawn(move || page_progress(msg_rx));

            loop {
                msg_tx.send(
                    format!(
                        "Retrieving posts for {}/{} page #{}",
                        self.target.service,
                        self.target.user,
                        (offset + 50) / 50
                    )
                )?;

                let mut posts: Vec<Post>;

                let url = Self::api_url_with_offset(&self.target, offset);

                loop {
                    sleep(API_DELAY).await;

                    let response = CLIENT.get(&url).send().await?;

                    match response.status() {
                        StatusCode::OK => {
                            posts = response.json().await?;
                            break;
                        }

                        StatusCode::TOO_MANY_REQUESTS => {
                            sleep(ARGS.rate_limit_backoff).await;
                        }

                        status => bail!("{url} returned unexpected status: {status}"),
                    }
                }

                if posts.is_empty() {
                    break;
                }

                self.posts.append(&mut posts);

                if self.target.page.is_some() {
                    break;
                }

                offset += 50;
            }
        }

        Ok(())
    }

    fn api_url_with_offset(target: &Target, offset: u32) -> String {
        format!(
            "https://{}.su/api/v1/{}/user/{}?o={offset}",
            target.site(),
            target.service,
            target.user
        )
    }

    fn init_files(&mut self) {
        self.post_count = self.posts.len();

        self.posts.drain(..).for_each(|post| {
            post.files()
                .into_iter()
                .for_each(|file| {
                    self.files.insert(file);
                });
        });
    }
}

#[derive(Deserialize, Clone, Default)]
struct Post {
    // coomer/kemono database id
    // id: String, // "1000537173"

    // service user name/id
    // user: String, // "paigetheuwulord"

    // service: Service, // "onlyfans"

    // post title
    // title: String, // "What an ass"
    #[serde(default)]
    file: PostFile,
    attachments: Vec<PostFile>,
}

impl Post {
    fn files(mut self) -> Vec<PostFile> {
        let mut files = vec![self.file];
        files.append(&mut self.attachments);
        files.retain(|pf| pf.path.is_some());
        files
    }
}

#[derive(Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord, Default, Hash)]
pub struct PostFile {
    // original file name
    // name: Option<String>, // "1242x2208_882b040faaac0e38fba20f4caadb2e59.jpg",

    // CDN file path (sha256.ext)
    path: Option<String>, // "/6e/6c/6e6cf84df44c1d091a2e36b6df77b098107c18831833de1e2e9c8207206f150b.jpg"
}

impl PostFile {
    pub fn to_url(&self, target: &Target) -> String {
        format!("https://{}.su/data{}", target.site(), self.path.as_ref().unwrap())
    }

    pub fn to_name(&self) -> String {
        PathBuf::from(self.path.as_ref().expect("get path from PostFile"))
            .file_name()
            .expect("get file name from CDN path")
            .to_string_lossy()
            .to_string()
    }

    pub fn to_temp_name(&self) -> String {
        self.to_name() + ".temp"
    }

    pub fn to_extension(&self, target: &Target) -> Option<String> {
        self.to_pathbuf(target)
            .extension()
            .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
    }

    pub fn to_pathbuf(&self, target: &Target) -> PathBuf {
        target.to_pathbuf(Some(&self.to_name()))
    }

    pub fn to_temp_pathbuf(&self, target: &Target) -> PathBuf {
        target.to_pathbuf(Some(&self.to_temp_name()))
    }

    pub fn to_hash(&self) -> String {
        self.to_name()[..64].to_string()
    }

    pub async fn open(&self, target: &Target) -> Result<File> {
        File::options()
            .append(true)
            .create(true)
            .truncate(false)
            .open(&self.to_temp_pathbuf(target)).await
            .with_context(|| format!("Failed to open temporary file: {}", self.to_temp_name()))
    }

    pub async fn hash(&self, target: &Target) -> Result<String> {
        sha256
            ::try_async_digest(&self.to_temp_pathbuf(target)).await
            .with_context(|| format!("hash tempfile: {}", self.to_temp_name()))
    }

    pub async fn exists(&self, target: &Target) -> Result<bool> {
        fs::try_exists(self.to_pathbuf(target)).await.with_context(||
            format!("check if file exists: {}", self.to_temp_name())
        )
    }

    pub async fn r#move(&self, target: &Target) -> Result<()> {
        fs::rename(self.to_temp_pathbuf(target), self.to_pathbuf(target)).await.with_context(|| {
            format!("rename tempfile to file: {} -> {}", self.to_temp_name(), self.to_name())
        })
    }

    pub async fn delete(&self, target: &Target) -> Result<()> {
        fs::remove_file(self.to_temp_pathbuf(target)).await.with_context(||
            format!("delete tempfile: {}", self.to_temp_name())
        )
    }

    pub async fn download(&self, target: &Target) -> Result<DownloadState> {
        if self.exists(target).await? {
            return Ok(DownloadState::Skip);
        }

        let rsize = self.remote_size(target).await?;

        let mut temp_file = self.open(target).await?;

        let isize = temp_file.seek(SeekFrom::End(0)).await?;

        let mut csize = isize;

        loop {
            if rsize == csize {
                break;
            }

            if let Err(err) = self.download_range(&mut temp_file, csize, target).await {
                let mut error = err.to_string();
                if let Some(source) = err.source() {
                    error.push('\n');
                    error.push_str(&source.to_string());
                }
                return Ok(DownloadState::Failure(csize - isize, error));
            }

            match temp_file.seek(SeekFrom::End(0)).await {
                Ok(cursor) => {
                    csize = cursor;
                }
                Err(err) => {
                    let mut error = err.to_string();
                    if let Some(source) = err.source() {
                        error.push('\n');
                        error.push_str(&source.to_string());
                    }
                    return Ok(DownloadState::Failure(csize - isize, error));
                }
            }
        }

        Ok({
            let dsize = csize - isize;

            if self.to_hash() == self.hash(target).await? {
                self.r#move(target).await?;
                DownloadState::Success(dsize)
            } else {
                self.delete(target).await?;
                DownloadState::Failure(
                    dsize,
                    format!("hash mismatch (deleted): {}", self.to_name())
                )
            }
        })
    }

    async fn download_range(&self, file: &mut File, start: u64, target: &Target) -> Result<()> {
        let url = self.to_url(target);

        loop {
            let response = CLIENT.get(&url)
                .header("Range", format!("bytes={start}-"))
                .send().await?;

            let status = response.status();

            if status == StatusCode::PARTIAL_CONTENT {
                let mut stream = response.bytes_stream();

                while let Some(Ok(bytes)) = stream.next().await {
                    file.write_all(&bytes).await?;
                }

                file.flush().await?;

                break Ok(());
            } else if status == StatusCode::NOT_FOUND {
                bail!("[{status}] download failed ({url})");
            } else if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
                sleep(ARGS.rate_limit_backoff).await;
            } else if status.is_server_error() {
                sleep(ARGS.server_error_delay).await;
            } else {
                bail!("[{status}] download failed: unexpected status code {url}");
            }
        }
    }

    pub async fn remote_size(&self, target: &Target) -> Result<u64> {
        fn size_error(status: StatusCode, message: &str, url: &str) -> Result<u64> {
            bail!("[{status}] failed to determine remote size: {message} ({url})")
        }

        let url = self.to_url(target);

        loop {
            let response = CLIENT.head(&url).send().await?;

            let status = response.status();

            if status == StatusCode::OK {
                return match response.content_length() {
                    Some(length) => Ok(length),
                    None => {
                        return size_error(status, "Content-Length header is not present", &url);
                    }
                };
            } else if status == StatusCode::NOT_FOUND {
                size_error(status, "file not found", &url)?;
            } else if status == StatusCode::FORBIDDEN || status == StatusCode::TOO_MANY_REQUESTS {
                sleep(ARGS.rate_limit_backoff).await;
            } else if status.is_server_error() {
                sleep(ARGS.server_error_delay).await;
            } else {
                size_error(status, "unexpected status code", &url)?;
            }
        }
    }
}
