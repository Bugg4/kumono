use crate::cli::ARGS;
use anyhow::{ Result, bail };
use regex::{ Captures, Regex };
use serde::Deserialize;
use strum_macros::{ Display, EnumString };
use std::{ fmt, path::PathBuf, process::exit, sync::LazyLock };

pub static TARGETS: LazyLock<Vec<Target>> = LazyLock::new(|| {
    let mut targets = Vec::new();

    for url in &ARGS.urls {
        match Target::from_url(url.strip_suffix('/').unwrap_or(url)) {
            Ok(target) => targets.push(target),
            Err(err) => eprintln!("{err}"),
        }
    }

    if targets.is_empty() {
        eprintln!("No valid target URLs were provided.");
        exit(1);
    }

    targets.sort();
    targets.dedup();
    targets
});

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    Creator {
        service: Service,
        user: String,
        subtype: SubType,
    },
    Discord {
        server: String,
        channel: Option<String>,
    },
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", match self {
            Target::Creator { service, user, subtype } =>
                format!("{service}/{user}{}", match subtype {
                    SubType::Post(p) => format!("/{p}"),
                    _ => String::new(),
                }),
            Target::Discord { server, channel } =>
                format!(
                    "discord/{server}{}",
                    channel.as_ref().map_or(String::new(), |c| format!("/{c}"))
                ),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SubType {
    PageOffset(usize),
    Post(String),
    None,
}

#[allow(unused)]
#[derive(Debug, Clone, Deserialize)]
struct Info {
    id: String, // "5564244",
    name: String, // "theobrobine",
    service: String, // "patreon",
    indexed: String, // "2020-09-30T06:13:38.348472",
    updated: String, // "2025-05-30T14:07:16.596525",
    public_id: Option<String>, // "theobrobine",
    relation_id: Option<u64>, // 8,
    has_chats: Option<bool>, // false
}

// static RE_LINKED: LazyLock<Regex> = LazyLock::new(|| {
//     Regex::new(
//         r"^(https://)?(coomer|kemono)\.su/(?<service>[a-z]+)/user/(?<user>[a-z|A-Z|0-9|\-|_|\.]+)/links$"
//     ).unwrap()
// });

static RE_CREATOR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(https://)?(coomer|kemono)\.su/(?<service>[a-z]+)/user/(?<user>[a-z|A-Z|0-9|\-|_|\.]+)$"
    ).unwrap()
});

static RE_PAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(https://)?(coomer|kemono)\.su/(?<service>[a-z]+)/user/(?<user>[a-z|A-Z|0-9|\-|_|\.]+)\?o=(?<offset>(0|50|[1-9]+(0|5)0))$"
    ).unwrap()
});

static RE_POST: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(https://)?(coomer|kemono)\.su/(?<service>[a-z]+)/user/(?<user>[a-z|A-Z|0-9|\-|_|\.]+)/post/(?<post>[a-z|A-Z|0-9|\-|_|\.]+)$"
    ).unwrap()
});

static RE_DISCORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^(https://)?kemono\.su/discord/server/(?<server>[0-9]{17,19})(/(?<channel>[0-9]{17,19}))?$"
    ).unwrap()
});

impl Target {
    pub fn service(&self) -> Service {
        match self {
            Target::Creator { service, .. } => *service,
            Target::Discord { .. } => Service::Discord,
        }
    }

    fn from_url(url: &str) -> Result<Self> {
        let capture = |re: &Regex| { re.captures(url).expect("get captures") };
        let extract = |caps: &Captures, name: &str| caps.name(name).map(|m| m.as_str().to_string());
        let extract_unwrap = |caps: &Captures, name: &str| {
            extract(caps, name).expect("extract values from captures")
        };

        Ok(
            if RE_CREATOR.is_match(url) {
                let caps = capture(&RE_CREATOR);
                Target::Creator {
                    service: extract_unwrap(&caps, "service").parse()?,
                    user: extract_unwrap(&caps, "user"),
                    subtype: SubType::None,
                }
            } else if RE_PAGE.is_match(url) {
                let caps = capture(&RE_PAGE);
                Target::Creator {
                    service: extract_unwrap(&caps, "service").parse()?,
                    user: extract_unwrap(&caps, "user"),
                    subtype: SubType::PageOffset(extract_unwrap(&caps, "offset").parse()?),
                }
            } else if RE_POST.is_match(url) {
                let caps = capture(&RE_POST);
                Target::Creator {
                    service: extract_unwrap(&caps, "service").parse()?,
                    user: extract_unwrap(&caps, "user"),
                    subtype: SubType::Post(extract_unwrap(&caps, "post")),
                }
            } else if RE_DISCORD.is_match(url) {
                let caps = capture(&RE_DISCORD);
                Target::Discord {
                    server: extract_unwrap(&caps, "server"),
                    channel: extract(&caps, "channel"),
                }
            } else {
                bail!("Invalid URL: {url}");
            }
        )
    }

    fn user(&self) -> String {
        match self {
            Target::Creator { user, .. } => user.to_string(),
            Target::Discord { server, .. } => server.to_string(),
        }
    }

    pub fn to_pathbuf(&self, file: Option<&str>) -> PathBuf {
        let mut path = PathBuf::from_iter([
            &ARGS.output_path,
            &self.service().to_string(),
            &self.user(),
        ]);

        if let Some(file) = file {
            path.push(file);
        }

        path
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, EnumString, Display)]
#[strum(ascii_case_insensitive, serialize_all = "lowercase")]
pub enum Service {
    Afdian,
    Boosty,
    CandFans,
    Discord,
    DlSite,
    Fanbox,
    Fansly,
    Fantia,
    Gumroad,
    OnlyFans,
    Patreon,
    SubscribeStar,
}

impl Service {
    pub fn site(self) -> &'static str {
        #[allow(clippy::enum_glob_use)]
        use Service::*;
        match self {
            CandFans | Fansly | OnlyFans => "coomer",
            _ => "kemono",
        }
    }
}
