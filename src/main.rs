use actix_cors::Cors;
use actix_web::{get, post, web, App, HttpResponse, HttpServer, Responder};
use chrono::format::StrftimeItems;
use chrono::prelude::*;
use chrono_tz::Tz;
use clap::Parser;
use env_logger::{Builder, Env};
use eyre::{eyre, Result};
use lazy_static::lazy_static;
use log::{debug, error, info, LevelFilter};
use regex::Regex;
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::env;
use std::io::Write;
use std::path::{Path,PathBuf};
use url::Url;

lazy_static! {
    static ref OBSIDIAN_BOOKMARK_PORT: String = env::var("OBSIDIAN_BOOKMARK_PORT").unwrap_or_else(|_| "65000".to_string());
    static ref TIMEZONE: Tz = "America/Los_Angeles".parse().expect("Invalid timezone");
    static ref YOUTUBE_API_KEY: String = env::var("YOUTUBE_API_KEY").expect("YOUTUBE_API_KEY not set in environment");
    static ref CHATGPT_API_KEY: String = env::var("CHATGPT_API_KEY").expect("CHATGPT_API_KEY not set in environment");
    static ref RESOLUTIONS: HashMap<&'static str, (usize, usize)> = {
        let mut m = HashMap::new();
        m.insert("nHD", (640, 360));
        m.insert("FWVGA", (854, 480));
        m.insert("qHD", (960, 540));
        m.insert("SD", (1280, 720));
        m.insert("WXGA", (1366, 768));
        m.insert("HD+", (1600, 900));
        m.insert("FHD", (1920, 1080));
        m.insert("WQHD", (2560, 1440));
        m.insert("QHD+", (3200, 1800));
        m.insert("4K", (3840, 2160));
        m.insert("5K", (5120, 2880));
        m.insert("8K", (7680, 4320));
        m.insert("16K", (15360, 8640));
        m
    };
    static ref SHORTS_RESOLUTIONS: HashMap<&'static str, (usize, usize)> = {
        let mut m = HashMap::new();
        m.insert("480p", (480, 854));
        m.insert("720p", (720, 1280));
        m.insert("1080p", (1080, 1920));
        m.insert("1440p", (1440, 2560));
        m.insert("2160p", (2160, 3840));
        m
    };
}

#[derive(Parser, Debug)]
struct Cli {
    #[arg(long, default_value = "65000")]
    port: u16,

    #[arg(long, default_value = "2")]
    workers: usize,

    #[arg(
        short,
        long,
        value_parser,
        default_value = "~/.config/obsidian-bookmark/obsidian-bookmark.yml"
    )]
    config: PathBuf,
}

#[derive(Serialize, Deserialize, Debug)]
struct Bookmark {
    title: String,
    url: String,
    folder: Option<String>,
    date: String,
}

#[derive(Deserialize, Debug, Clone, Default)]
struct Frontmatter {
    date: String,
    day: String,
    time: String,
    tags: Vec<String>,
    url: String,
    author: String,
    published: String,
}

impl Frontmatter {
    fn merge(&self, actual: &Frontmatter) -> Frontmatter {
        Frontmatter {
            date: if actual.date.is_empty() { self.date.clone() } else { actual.date.clone() },
            day: if actual.day.is_empty() { self.day.clone() } else { actual.day.clone() },
            time: if actual.time.is_empty() { self.time.clone() } else { actual.time.clone() },
            tags: if actual.tags.is_empty() { self.tags.clone() } else { actual.tags.clone() },
            url: if actual.url.is_empty() { self.url.clone() } else { actual.url.clone() },
            author: if actual.author.is_empty() { self.author.clone() } else { actual.author.clone() },
            published: if actual.published.is_empty() { self.published.clone() } else { actual.published.clone() },
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
struct Config {
    vault: PathBuf,
    frontmatter: Frontmatter,
    links: Vec<Link>,
}

impl Config {
    fn complete_frontmatter(frontmatter: Frontmatter) -> Frontmatter {
        Frontmatter {
            date: frontmatter.date,
            day: frontmatter.day,
            time: frontmatter.time,
            tags: frontmatter.tags,
            url: frontmatter.url,
            author: frontmatter.author,
            published: frontmatter.published,
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
struct Link {
    name: String,
    regex: String,
    resolution: String,
    folder: String,
}

#[derive(Debug)]
struct VideoMetadata {
    #[allow(dead_code)]
    id: String,
    title: String,
    description: String,
    channel: String,
    published_at: String,
    tags: Vec<String>,
}

enum LinkType {
    Shorts(String, String, usize, usize),
    YouTube(String, String, usize, usize),
    WebLink(String, String, usize, usize),
}

impl LinkType {
    fn from_url(url: &str, config: &Config) -> Result<Self> {
        debug!("LinkType::from_url: url={} config={:?}", url, config);
        let mut default_link = None;

        for link in &config.links {
            let regex = Regex::new(&link.regex)?;
            debug!("before regex.is_match on {:?}", url);
            if regex.is_match(url) {
                let (width, height) = get_resolution(&link.name, config)?;
                if link.name == "default" {
                    default_link = Some(Self::WebLink(url.to_string(), link.folder.clone(), width, height));
                    continue;
                }
                return Ok(match link.name.as_str() {
                    "shorts" => Self::Shorts(url.to_string(), link.folder.clone(), width, height),
                    "youtube" => Self::YouTube(url.to_string(), link.folder.clone(), width, height),
                    _ => Self::WebLink(url.to_string(), link.folder.clone(), width, height),
                });
            }
        }

        if let Some(default_link) = default_link {
            return Ok(default_link);
        }

        Err(eyre!("Invalid URL format"))
    }
}

fn expanduser<T: AsRef<str>>(path: T) -> PathBuf {
    let expanded_path_str = shellexpand::tilde(path.as_ref());
    PathBuf::from(expanded_path_str.into_owned())
}

fn today() -> (String, String, String) {
    debug!("today");
    let now = Utc::now().with_timezone(&*TIMEZONE);

    let date_format = StrftimeItems::new("%Y-%m-%d");
    let day_format = StrftimeItems::new("%a");
    let time_format = StrftimeItems::new("%H:%M");

    let formatted_date = now.format_with_items(date_format.clone()).to_string();
    let formatted_day = now.format_with_items(day_format.clone()).to_string();
    let formatted_time = now.format_with_items(time_format.clone()).to_string();

    (formatted_date, formatted_day, formatted_time)
}

fn get_resolution(link_name: &str, config: &Config) -> Result<(usize, usize)> {
    debug!("get_resolution: link_name={} config={:?}", link_name, config);
    let resolution_key = config
        .links
        .iter()
        .find(|link| link.name == link_name)
        .ok_or_else(|| eyre!("Link type '{}' not found in config", link_name))?
        .resolution
        .as_str();

    match link_name {
        "shorts" => SHORTS_RESOLUTIONS
            .get(resolution_key)
            .copied()
            .ok_or_else(|| eyre!("Resolution not found for shorts")),
        "youtube" | _ => RESOLUTIONS
            .get(resolution_key)
            .copied()
            .ok_or_else(|| eyre!("Resolution not found for {}", link_name)),
    }
}

fn get_field_value<T: Clone + Default + PartialEq>(incoming: &T, default: &T, fallback: T) -> T {
    if incoming != &T::default() {
        incoming.clone()
    } else if default != &T::default() {
        default.clone()
    } else {
        fallback
    }
}

fn format_frontmatter(frontmatter: &Frontmatter, url: &str, author: &str, tags: &[String], published: &str, default_frontmatter: &Frontmatter) -> Frontmatter {
    debug!(
        "format_frontmatter: frontmatter={:?} url={} author={} tags={:?}",
        frontmatter, url, author, tags
    );

    let (current_date, current_day, current_time) = today();

    let date = get_field_value(&frontmatter.date, &default_frontmatter.date, current_date);
    let day = get_field_value(&frontmatter.day, &default_frontmatter.day, current_day);
    let time = get_field_value(&frontmatter.time, &default_frontmatter.time, current_time);

    let mut combined_tags: HashSet<String> = tags.iter().cloned().collect();
    combined_tags.extend(default_frontmatter.tags.iter().cloned());
    let mut tags: Vec<String> = combined_tags.into_iter().collect();
    tags.sort();

    let url = get_field_value(&frontmatter.url, &default_frontmatter.url, url.to_string());
    let author = get_field_value(&frontmatter.author, &default_frontmatter.author, author.to_string());
    let published = get_field_value(&frontmatter.published, &default_frontmatter.published, published.to_string());

    Frontmatter {
        date,
        day,
        time,
        tags,
        url,
        author,
        published,
    }
}

fn sanitize_tag(tag: &str) -> String {
    debug!("sanitize_tag: tag={}", tag);
    tag.replace('\'', "")
        .chars()
        .map(|c| if c.is_alphanumeric() || c.is_whitespace() { c } else { '-' })
        .collect::<String>()
        .replace(' ', "-")
        .to_lowercase()
}

fn sanitize_filename(title: &str) -> Result<String> {
    debug!("sanitize_filename: title={}", title);
    let re = Regex::new(r"\s{2,}").map_err(|e| eyre!("Failed to compile regex: {}", e))?;
    let sanitized_title = title
        .chars()
        .filter(|c| c.is_alphanumeric() || c.is_whitespace() || *c == '-')
        .collect::<String>();
    Ok(re.replace_all(&sanitized_title, " ").to_string())
}

fn extract_video_id(url: &str) -> Result<String> {
    debug!("extract_video_id: url={}", url);
    let pattern = Regex::new(r#"(youtu\.be/|youtube\.com/(watch\?(.*&)?v=|(embed|v|shorts)/))([^?&">]+)"#)
        .map_err(|e| eyre!("Failed to compile regex: {}", e))?;

    pattern
        .captures(url)
        .and_then(|caps| caps.get(5))
        .map(|m| m.as_str().to_string())
        .ok_or_else(|| eyre!("Failed to extract video ID from URL"))
}

async fn fetch_video_metadata(api_key: &str, video_id: &str) -> Result<VideoMetadata> {
    debug!("fetch_video_metadata: api_key={} video_id={}", api_key, video_id);
    let url = format!(
        "https://www.googleapis.com/youtube/v3/videos?id={video_id}&part=snippet&key={api_key}"
    );

    let response = reqwest::get(&url).await?.json::<serde_json::Value>().await?;

    if response["items"].as_array().unwrap_or(&Vec::new()).is_empty() {
        return Err(eyre!("Video metadata not found for video_id={}", video_id));
    }

    let snippet = &response["items"][0]["snippet"];
    Ok(VideoMetadata {
        id: video_id.to_string(),
        title: snippet["title"].as_str().unwrap_or_default().to_string(),
        description: snippet["description"].as_str().unwrap_or_default().to_string(),
        channel: snippet["channelTitle"].as_str().unwrap_or_default().to_string(),
        published_at: snippet["publishedAt"].as_str().unwrap_or_default().to_string(),
        tags: snippet["tags"]
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .filter_map(|tag| tag.as_str())
            .map(String::from)
            .collect(),
    })
}

fn generate_embed_code(video_id: &str, width: usize, height: usize) -> String {
    debug!(
        "generate_embed_code: video_id={} width={} height={}",
        video_id, width, height
    );
    format!(
        "<iframe width=\"{width}\" height=\"{height}\" src=\"https://www.youtube.com/embed/{video_id}\" frameborder=\"0\" allowfullscreen></iframe>"
    )
}

fn generate_image_embed_code(img_url: &str, width: usize, height: usize) -> String {
    format!(
        "<img src=\"{img_url}\" width=\"{width}\" height=\"{height}\" alt=\"Image\" />"
    )
}

fn extract_title_and_tags(text: &str) -> Result<(String, Vec<String>)> {
    let mut modified_text = text.to_string();

    // Clean the title by removing specific prefixes and suffixes
    if modified_text.starts_with("(2) ") {
        modified_text = modified_text[4..].to_string(); // Remove the prefix "(2) "
    }
    if let Some(pos) = modified_text.rfind(" - YouTube") {
        modified_text = modified_text[..pos].to_string(); // Remove the suffix " - YouTube"
    }

    let re = Regex::new(r"(?i)\(1\)\s*|#(\w+)").map_err(|e| eyre!("Failed to compile regex: {}", e))?;
    let tags: Vec<String> = re
        .captures_iter(&modified_text)
        .filter_map(|cap| cap.get(1).map(|m| m.as_str().to_string()))
        .collect();
    let mut title = re.replace_all(&modified_text, "").to_string();
    // Fix: Remove extra spaces from the title
    title = title.split_whitespace().collect::<Vec<_>>().join(" ");

    Ok((title.trim().to_string(), tags))
}

fn create_markdown_file(
    title: &str,
    description: &str,
    embed_code: &str,
    url: &str,
    author: &str,
    tags: &[String],
    vault_path: &Path,
    folder: Option<String>,
    frontmatter: &Frontmatter,
) -> Result<()> {
    info!("create_markdown_file: title={} description={} embed_code={} url={} author={} tags={:?} vault_path={} folder={:?} frontmatter={:?}", title, description, embed_code, url, author, tags, vault_path.display(), folder, frontmatter);
    let vault_path_str = vault_path
        .to_str()
        .ok_or_else(|| eyre!("Failed to convert vault path to string"))?;
    let vault_path_expanded = expanduser(vault_path_str);

    let folder_path = if let Some(folder) = folder {
        vault_path_expanded.join(folder)
    } else {
        vault_path_expanded
    };

    std::fs::create_dir_all(&folder_path)
        .map_err(|e| eyre!("Failed to create directory: {:?} with error {}", folder_path, e))?;

    let file_name = sanitize_filename(title)?;
    let file_path = folder_path.join(file_name + ".md");

    info!("file_path={:?}", file_path);

    let mut file = std::fs::File::create(&file_path)
        .map_err(|e| eyre!("Failed to create markdown file: {:?} with error {}", file_path, e))?;

    writeln!(file, "---")?;
    writeln!(file, "date: {}", frontmatter.date)?;
    writeln!(file, "day: {}", frontmatter.day)?;
    writeln!(file, "time: {}", frontmatter.time)?;
    writeln!(file, "tags:")?;
    for tag in &frontmatter.tags {
        writeln!(file, "  - {}", sanitize_tag(tag))?;
    }
    writeln!(file, "url: {}", frontmatter.url)?;
    writeln!(file, "author: {}", frontmatter.author)?;
    writeln!(file, "published: {}", frontmatter.published)?;
    writeln!(file, "type: link")?;
    writeln!(file, "---\n")?;

    write!(
        file,
        "{}\n\n## Description\n{}",
        embed_code, description
    )
    .map_err(|e| eyre!("Failed to write to markdown file: {}", e))
}

async fn download_webpage(url: &str) -> Result<String> {
    let response = reqwest::get(url).await?;
    let content = response.text().await?;
    Ok(content)
}

fn extract_data_from_webpage(content: &str) -> Result<(String, String, String, String, String, Vec<String>)> {
    let document = Html::parse_document(content);

    let title_selector = Selector::parse("title").map_err(|e| eyre!("Failed to compile selector: {}", e))?;
    let title = document
        .select(&title_selector)
        .next()
        .map_or(String::new(), |e| e.inner_html());

    let meta_selector = Selector::parse("meta[name='description']").map_err(|e| eyre!("Failed to compile selector: {}", e))?;
    let summary = document
        .select(&meta_selector)
        .next()
        .map_or(String::new(), |e| e.value().attr("content").unwrap_or("").to_string());

    let author_selectors = [
        Selector::parse("meta[name='author']").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
        Selector::parse("meta[property='article:author']").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
        Selector::parse(".author").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
        Selector::parse("[itemprop='author']").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
    ];
    let mut author = "Not specified".to_string();
    for selector in &author_selectors {
        if let Some(element) = document.select(selector).next() {
            author = element.text().collect::<Vec<_>>().join(" ");
            break;
        }
    }

    let published_selectors = [
        Selector::parse("meta[property='article:published_time']").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
        Selector::parse("meta[name='publication_date']").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
        Selector::parse("time[datetime]").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
        Selector::parse("[itemprop='datePublished']").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
    ];
    let mut published = String::new();
    for selector in &published_selectors {
        if let Some(element) = document.select(selector).next() {
            published = element.value().attr("content").unwrap_or("").to_string();
            if published.is_empty() {
                published = element.text().collect::<Vec<_>>().join(" ");
            }
            break;
        }
    }

    let image_selector = Selector::parse("meta[property='og:image']").map_err(|e| eyre!("Failed to compile selector: {}", e))?;
    let image = document
        .select(&image_selector)
        .next()
        .map_or(String::new(), |e| e.value().attr("content").unwrap_or("").to_string());

    let tag_selectors = [
        Selector::parse("meta[name='keywords']").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
        Selector::parse(".tags").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
        Selector::parse(".tag").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
        Selector::parse(".keywords").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
        Selector::parse("[itemprop='keywords']").map_err(|e| eyre!("Failed to compile selector: {}", e))?,
    ];

    let mut tags = Vec::new();

    for selector in &tag_selectors {
        if let Some(element) = document.select(selector).next() {
            if let Some(content) = element.value().attr("content") {
                tags.extend(content.split(',').map(|tag| tag.trim().to_string()));
            } else {
                tags.extend(element.text().map(|tag| tag.trim().to_string()));
            }
        }
    }

    tags = tags.into_iter().filter(|tag| !tag.is_empty()).collect();
    tags.sort();
    tags.dedup();

    Ok((title, summary, author, published, image, tags))
}

async fn fetch_and_summarize_url_with_chatgpt(
    url: &str,
) -> Result<(String, String, String, String, String, Vec<String>)> {
    let content = download_webpage(url).await?;
    let (title, summary, author, published, image, tags) = extract_data_from_webpage(&content)?;

    debug!("Fetched content from URL: {}", url);
    debug!(
        "Extracted data - Title: {}, Summary: {}, Author: {}, Published: {}, Image: {}, Tags: {:?}",
        title, summary, author, published, image, tags
    );

    let prompt = format!(
        "Please provide a JSON object with the following details about the URL: {url}.
        - Title: {title}
        - Summary: {summary}
        - Author: {author}
        - Published: {published}
        - Main Image URL: {image}
        - Tags: {tags:?}

        The JSON object should include:
        - 'title': The title of the article
        - 'summary': A detailed summary of the article (at least 100 words)
        - 'author': The author of the article
        - 'published': The date of the publication
        - 'main_image_url': The main image URL of the article
        - 'tags': Relevant tags for the article

        URL: {url}"
    );

    debug!("Prompt for ChatGPT: {}", prompt);

    let client = reqwest::Client::new();
    let request_body = json!({
        "model": "gpt-3.5-turbo",
        "messages": [
            {"role": "system", "content": "You are a helpful assistant."},
            {"role": "user", "content": prompt}
        ]
    });

    let response = client
        .post("https://api.openai.com/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", CHATGPT_API_KEY.as_str()))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .await?;

    debug!("Response from ChatGPT: {:?}", response);

    if response.status() == 200 {
        let response_body = response.json::<serde_json::Value>().await?;
        let assistant_reply = &response_body["choices"][0]["message"]["content"];

        debug!("Assistant reply: {:?}", assistant_reply);

        assistant_reply.as_str().map_or_else(
            || {
                error!("Failed to parse ChatGPT response: {:?}", response_body);
                Err(eyre!("Failed to parse ChatGPT response"))
            },
            |reply_str| {
                // Remove code block markers (```json ... ```)
                let json_str = reply_str
                    .trim()
                    .strip_prefix("```json")
                    .unwrap_or(reply_str)
                    .strip_suffix("```")
                    .unwrap_or(reply_str)
                    .trim();
                debug!("Extracted JSON string: {}", json_str);

                match serde_json::from_str::<serde_json::Value>(json_str) {
                    Ok(parsed) => {
                        debug!("Parsed JSON from assistant reply: {:?}", parsed);

                        let (current_date, _, _) = today();
                        let title = parsed["title"]
                            .as_str()
                            .unwrap_or(&format!("No Title {current_date}"))
                            .to_string();
                        let summary = parsed["summary"].as_str().unwrap_or_default().to_string();
                        let author = parsed["author"].as_str().unwrap_or_default().to_string();
                        let published = parsed["published"].as_str().unwrap_or_default().to_string();
                        let image = parsed["main_image_url"].as_str().unwrap_or_default().to_string();
                        let tags = parsed["tags"].as_array().map_or_else(Vec::new, |arr| {
                            arr.iter().filter_map(|tag| tag.as_str().map(String::from)).collect()
                        });

                        debug!("Final extracted data - Title: {}, Summary: {}, Author: {}, Published: {}, Image: {}, Tags: {:?}", title, summary, author, published, image, tags);

                        Ok((title, summary, author, published, image, tags))
                    }
                    Err(e) => {
                        error!("Failed to parse extracted JSON string: {}", e);
                        Err(eyre!("Failed to parse ChatGPT response"))
                    }
                }
            },
        )
    } else {
        let error_text = response.text().await?;
        error!("Error response from ChatGPT: {}", error_text);
        Err(eyre!("Error: {}", error_text))
    }
}

async fn handle_shorts_url(
    url: &str,
    title: &str,
    folder: Option<String>,
    width: usize,
    height: usize,
    config: &Config,
) -> Result<()> {
    info!(
        "handle_shorts_url: url={}, title={} folder={:?}, width={} height={}, config={:?}",
        url, title, folder, width, height, config
    );
    let video_id = extract_video_id(url)?;
    let metadata = fetch_video_metadata(&YOUTUBE_API_KEY, &video_id).await?;
    let embed_code = generate_embed_code(&video_id, width, height);

    let (metadata_title, metadata_tags) = extract_title_and_tags(&metadata.title)?;
    let (title, tags) = extract_title_and_tags(title)?;

    let final_title = if title.is_empty() { metadata_title } else { title };

    let mut combined_tags: HashSet<String> = HashSet::new();
    combined_tags.extend(tags);
    combined_tags.extend(metadata_tags);
    combined_tags.extend(metadata.tags);
    let combined_tags: Vec<String> = combined_tags.into_iter().collect();

    let frontmatter = config.frontmatter.merge(&format_frontmatter(
        &config.frontmatter,
        url,
        &metadata.channel,
        &combined_tags,
        &metadata.published_at,
        &config.frontmatter,
    ));

    create_markdown_file(
        &final_title,
        &metadata.description,
        &embed_code,
        url,
        &metadata.channel,
        &combined_tags,
        &config.vault,
        folder,
        &frontmatter,
    )
}

async fn handle_youtube_url(
    url: &str,
    title: &str,
    folder: Option<String>,
    width: usize,
    height: usize,
    config: &Config,
) -> Result<()> {
    info!(
        "handle_youtube_url: url={}, title={} folder={:?}, width={} height={}, config={:?}",
        url, title, folder, width, height, config
    );
    let video_id = extract_video_id(url)?;
    let metadata = fetch_video_metadata(&YOUTUBE_API_KEY, &video_id).await?;
    let embed_code = generate_embed_code(&video_id, width, height);

    let (metadata_title, metadata_tags) = extract_title_and_tags(&metadata.title)?;
    let (title, tags) = extract_title_and_tags(title)?;

    let final_title = if title.is_empty() { metadata_title } else { title };

    let mut combined_tags: HashSet<String> = HashSet::new();
    combined_tags.extend(tags);
    combined_tags.extend(metadata_tags);
    combined_tags.extend(metadata.tags);
    let combined_tags: Vec<String> = combined_tags.into_iter().collect();

    let frontmatter = config.frontmatter.merge(&format_frontmatter(
        &config.frontmatter,
        url,
        &metadata.channel,
        &combined_tags,
        &metadata.published_at,
        &config.frontmatter,
    ));

    create_markdown_file(
        &final_title,
        &metadata.description,
        &embed_code,
        url,
        &metadata.channel,
        &combined_tags,
        &config.vault,
        folder,
        &frontmatter,
    )
}

async fn handle_weblink_url(
    url: &str,
    title: &str,
    folder: Option<String>,
    width: usize,
    height: usize,
    config: &Config,
) -> Result<()> {
    info!(
        "handle_weblink_url: url={}, title={} folder={:?}, width={} height={}, config={:?}",
        url, title, folder, width, height, config
    );
    let (fetched_title, summary, author, published, image, fetched_tags) =
        fetch_and_summarize_url_with_chatgpt(url).await?;
    let embed_code = if image.is_empty() {
        String::new()
    } else {
        generate_image_embed_code(&image, width, height)
    };

    let (metadata_title, metadata_tags) = extract_title_and_tags(&fetched_title)?;
    let (title, tags) = extract_title_and_tags(title)?;

    let final_title = if title.is_empty() { metadata_title } else { title };

    let mut combined_tags: HashSet<String> = HashSet::new();
    combined_tags.extend(tags);
    combined_tags.extend(metadata_tags);
    combined_tags.extend(fetched_tags);
    let combined_tags: Vec<String> = combined_tags.into_iter().collect();

    let frontmatter = config.frontmatter.merge(&format_frontmatter(
        &config.frontmatter,
        url,
        &author,
        &combined_tags,
        &published,
        &config.frontmatter,
    ));

    create_markdown_file(
        &final_title,
        &summary,
        &embed_code,
        url,
        &author,
        &combined_tags,
        &config.vault,
        folder,
        &frontmatter,
    )
}

fn remove_utm_source(url: &str) -> Result<String> {
    debug!("remove_utm_source: url={}", url);
    let mut parsed_url = Url::parse(url).map_err(|e| eyre!("Failed to parse URL: {}", e))?;
    let mut query_pairs = parsed_url.query_pairs().into_owned().collect::<Vec<(String, String)>>();
    query_pairs.retain(|(key, _)| key != "utm_source");
    parsed_url.query_pairs_mut().clear().extend_pairs(query_pairs);

    if parsed_url.query_pairs().count() == 0 {
        parsed_url.set_query(None);
    }

    Ok(parsed_url.to_string())
}

async fn handle_url(url: &str, title: &str, folder: Option<String>, config: &Config) -> Result<()> {
    debug!(
        "handle_url: url={} title={} folder={:?} config={:?}",
        url, title, folder, config
    );
    let url = remove_utm_source(url)?;
    debug!("utm_source removed url={}", url);
    match LinkType::from_url(&url, config)? {
        LinkType::Shorts(url, default_folder, width, height) => {
            handle_shorts_url(&url, title, folder.or(Some(default_folder)), width, height, config).await
        }
        LinkType::YouTube(url, default_folder, width, height) => {
            handle_youtube_url(&url, title, folder.or(Some(default_folder)), width, height, config).await
        }
        LinkType::WebLink(url, default_folder, width, height) => {
            handle_weblink_url(&url, title, folder.or(Some(default_folder)), width, height, config).await
        }
    }
}

#[post("/bookmark")]
async fn bookmark(bookmark: web::Json<Bookmark>, config: web::Data<Config>) -> impl Responder {
    info!("bookmark:");
    info!("- title: {}", bookmark.title);
    info!("- url: {}", bookmark.url);

    match handle_url(&bookmark.url, &bookmark.title, bookmark.folder.clone(), &config).await {
        Ok(()) => HttpResponse::Ok().json(serde_json::json!({"status": "success"})),
        Err(e) => {
            error!("Failed to process bookmark: {:?}", e);
            HttpResponse::InternalServerError().json(serde_json::json!({"status": "error", "message": e.to_string()}))
        }
    }
}

#[get("/health")]
async fn health() -> impl Responder {
    debug!("/health Ok");
    HttpResponse::Ok().body("OK")
}

fn init_logger() {
    let env = Env::default().filter_or("RUST_LOG", "info");
    let mut builder = Builder::from_env(env);
    builder.filter(None, LevelFilter::Info);
    if let Ok(rust_log) = env::var("RUST_LOG") {
        builder.filter(Some("obsidian_bookmark"), rust_log.parse().unwrap_or(LevelFilter::Info));
    } else {
        builder.filter(Some("obsidian_bookmark"), LevelFilter::Info);
    }
    builder.init();
}

fn load_config(config_path: &Path) -> Result<Config> {
    debug!("load_config: config_path={}", config_path.display());
    let config_path_str = config_path
        .to_str()
        .ok_or_else(|| eyre!("Failed to convert config path to string"))?;
    let config_path_expanded = expanduser(config_path_str);
    let config_str =
        std::fs::read_to_string(config_path_expanded).map_err(|e| eyre!("Failed to read config file: {}", e))?;
    let mut config: Config =
        serde_yaml::from_str(&config_str).map_err(|e| eyre!("Failed to parse config file: {}", e))?;
    config.frontmatter = Config::complete_frontmatter(config.frontmatter);
    Ok(config)
}

#[tokio::main]
async fn main() -> Result<()> {
    init_logger();

    let cli = Cli::parse();
    let port = OBSIDIAN_BOOKMARK_PORT.parse::<u16>().unwrap_or(cli.port);
    info!("Starting server with POST endpoint: /process_bookmark");
    info!("Starting server on port: {}", cli.port);

    let config = load_config(&cli.config)?;

    let server = HttpServer::new(move || {
        info!("Setting up the Actix app with CORS and services");
        let cors = Cors::default()
            .allow_any_origin()
            .allow_any_method()
            .allow_any_header()
            .max_age(3600);
        App::new()
            .app_data(web::Data::new(config.clone()))
            //.wrap(Cors::permissive())
            .wrap(cors)
            .service(health)
            .service(bookmark)
    })
    .workers(cli.workers);

    info!("Binding server to 0.0.0.0:{}", port);
    server.bind(("0.0.0.0", port))?.run().await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn load_test_config() -> Config {
        let config_path = shellexpand::tilde("~/.config/obsidian-bookmark/obsidian-bookmark.yml");
        let config_path = Path::new(config_path.as_ref());

        load_config(config_path).expect("Failed to load config")
    }

    #[tokio::test]
    async fn test_malformed_frontmatter_handling() -> Result<()> {
        let config = load_test_config();

        let malformed_frontmatter = Frontmatter {
            date: "2024-06-14".to_string(),
            day: "Fri".to_string(),
            time: "23:41".to_string(),
            tags: vec![
                "anatomy-of-a-play".to_string(),
                "mills-concept-football".to_string(),
                "national-football-league".to_string(),
                "weekly-spiral".to_string(),
                "football-basics".to_string(),
                "mills".to_string(),
                "american-football--sport-".to_string(),
                "passing-game".to_string(),
                "american-football".to_string(),
                "mills-concept".to_string(),
                "football-101".to_string(),
            ],
            url: "https://www.youtube.com/watch?v=7sgCH4U7rjU&t=32s".to_string(),
            author: "Weekly Spiral".to_string(),
            published: "2021-08-16T15:29:39Z".to_string(),
        };

        let expected_output = r#"---
    date: 2024-06-14
    day: Fri
    time: 23:41
    tags:
    - american-football
    - american-football--sport-
    - anatomy-of-a-play
    - football-101
    - football-basics
    - mills
    - mills-concept
    - mills-concept-football
    - national-football-league
    - passing-game
    - weekly-spiral
    url: https://www.youtube.com/watch?v=7sgCH4U7rjU&t=32s
    author: Weekly Spiral
    published: 2021-08-16T15:29:39Z
    type: link
    ---"#;

        let formatted_frontmatter = format_frontmatter(
            &malformed_frontmatter,
            &malformed_frontmatter.url,
            &malformed_frontmatter.author,
            &malformed_frontmatter.tags,
            &malformed_frontmatter.published,
            &config.frontmatter,
        );

        let frontmatter_str = format!(
            "---\ndate: {}\nday: {}\ntime: {}\ntags:\n",
            formatted_frontmatter.date, formatted_frontmatter.day, formatted_frontmatter.time
        );

        let mut tags_str = String::new();
        for tag in &formatted_frontmatter.tags {
            tags_str.push_str(&format!("  - {}\n", sanitize_tag(tag)));
        }

        let final_output = format!(
            "{}{}url: {}\nauthor: {}\npublished: {}\ntype: link\n---",
            frontmatter_str,
            tags_str,
            formatted_frontmatter.url,
            formatted_frontmatter.author,
            formatted_frontmatter.published
        );

        // Improved output for debugging
        println!("Left:\n{}\n", final_output);
        println!("Right:\n{}\n", expected_output);

        assert_eq!(final_output, expected_output, "Malformed frontmatter should be formatted correctly");

        Ok(())
    }

    #[tokio::test]
    async fn test_youtube_shorts_identification() -> Result<()> {
        let config = load_test_config();
        let shorts_urls = vec![
            "https://www.youtube.com/shorts/gGrqPbb6fuM",
            "https://www.youtube.com/shorts/FjkS5rjNq-A",
        ];
        for url in shorts_urls {
            let link_type = LinkType::from_url(url, &config)?;
            assert!(matches!(link_type, LinkType::Shorts(..)));
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_youtube_url_identification() -> Result<()> {
        let config = load_test_config();

        let urls = vec![
            "https://www.youtube.com/watch?v=y4evLICF8kk",
            "https://www.youtube.com/watch?v=U3HndX2QnSo",
            "https://youtu.be/EkDxsQRbIwoA",
            "https://youtu.be/m7lnIdudEy8?si=VE-14Y1Sk93RdA5u",
        ];

        for url in urls {
            let link_type = LinkType::from_url(url, &config)?;
            assert!(matches!(link_type, LinkType::YouTube(..)));
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_weblink_identification() -> Result<()> {
        let config = load_test_config();

        let weblink_urls = vec!["https://parrot.ai/", "https://pdfgpt.io/"];

        for url in weblink_urls {
            let link_type = LinkType::from_url(url, &config)?;
            assert!(matches!(link_type, LinkType::WebLink(..)));
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_shorts_url_format() -> Result<()> {
        let config = load_test_config();
        let invalid_shorts_url = "https://www.youtube.com/notshorts/gGrqPbb6fuM";
        let link_type = LinkType::from_url(invalid_shorts_url, &config)?;
        assert!(
            matches!(link_type, LinkType::WebLink(..)),
            "Expected a WebLink for invalid Shorts URL format"
        );
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_youtube_url_format() -> Result<()> {
        let config = load_test_config();
        let invalid_youtube_url = "https://www.notyoutube.com/watch?v=y4evLICF8kk";
        let link_type = LinkType::from_url(invalid_youtube_url, &config)?;
        assert!(
            matches!(link_type, LinkType::WebLink(..)),
            "Expected a WebLink for invalid YouTube URL format"
        );
        Ok(())
    }

    #[test]
    fn test_generate_embed_code_non_integer() {
        let video_id = "y4evLICF8kk";
        let embed_code = generate_embed_code(video_id, 0, 0);
        assert!(
            embed_code.contains("width=\"0\""),
            "Embed code should contain width=\"0\""
        );
        assert!(
            embed_code.contains("height=\"0\""),
            "Embed code should contain height=\"0\""
        );
    }

    #[tokio::test]
    async fn test_create_markdown_special_characters() -> Result<()> {
        let title = "Test: Special/Characters?*";
        let description = "A test video.";
        let embed_code = "<iframe...></iframe>";
        let url = "https://www.example.com";
        let author = "Test Channel";
        let tags = vec![String::from("test")];
        let config = load_test_config();

        let result = create_markdown_file(
            title,
            description,
            embed_code,
            url,
            author,
            &tags,
            &config.vault,
            Some("test_folder".to_string()),
            &config.frontmatter,
        );

        assert!(
            result.is_ok(),
            "Failed to create markdown file with special characters in title"
        );
        Ok(())
    }

    #[test]
    fn test_extract_title_and_tags() -> Result<()> {
        let text = "(1) Test title with #tag1 and #tag2";
        let (title, tags) = extract_title_and_tags(text)?;
        assert_eq!(title, "Test title with and");
        assert_eq!(tags, vec!["tag1".to_string(), "tag2".to_string()]);
        Ok(())
    }

    #[test]
    fn test_extract_title_and_tags_no_prefix() -> Result<()> {
        let text = "Test title with #tag1 and #tag2";
        let (title, tags) = extract_title_and_tags(text)?;
        assert_eq!(title, "Test title with and");
        assert_eq!(tags, vec!["tag1".to_string(), "tag2".to_string()]);
        Ok(())
    }
}
