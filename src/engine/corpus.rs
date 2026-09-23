use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tree_sitter::{Language, Node, Parser};

use crate::engine::content::{CodeLanguage, Snippet, WordLanguage, WordListSize};

const MONKEYTYPE_COMMIT: &str = "e113dff1cfc27cc624f47ac9899d6e287c3fc33f";
const CACHE_LIMIT: u64 = 500 * 1024 * 1024;
const ALLOWED_LICENSES: &[&str] = &["mit", "apache-2.0", "bsd-2-clause", "bsd-3-clause", "isc"];

#[derive(Debug)]
pub enum LoadRequest {
    Words(WordLanguage, WordListSize),
    Code(CodeLanguage),
    Refresh(CodeLanguage),
}

#[derive(Debug)]
pub enum LoadResult {
    Words(Vec<String>),
    Code(Vec<Snippet>),
}

#[derive(Debug, Deserialize)]
struct MonkeytypeList {
    words: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct Repository {
    full_name: String,
    default_branch: String,
    html_url: String,
    license: Option<License>,
}

#[derive(Debug, Clone, Deserialize)]
struct License {
    spdx_id: String,
}

#[derive(Debug, Deserialize)]
struct SearchResponse {
    items: Vec<Repository>,
}

#[derive(Debug, Deserialize)]
struct TreeResponse {
    tree: Vec<TreeEntry>,
}

#[derive(Debug, Deserialize)]
struct TreeEntry {
    path: String,
    #[serde(rename = "type")]
    kind: String,
    size: Option<u64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ProcessedRepos {
    names: HashSet<String>,
}

pub fn load(request: LoadRequest) -> Result<LoadResult> {
    let result = match request {
        LoadRequest::Words(language, size) => LoadResult::Words(load_words(language, size)?),
        LoadRequest::Code(language) => {
            let cached = read_snippets(language)?;
            if cached.is_empty() {
                LoadResult::Code(refresh_snippets(language)?)
            } else {
                refresh_in_background_if_due(language);
                LoadResult::Code(cached)
            }
        }
        LoadRequest::Refresh(language) => LoadResult::Code(refresh_snippets(language)?),
    };
    enforce_cache_limit()?;
    Ok(result)
}

pub fn cache_root() -> PathBuf {
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("typeforge")
}

fn load_words(language: WordLanguage, size: WordListSize) -> Result<Vec<String>> {
    let directory = cache_root().join("wordlists");
    let path = directory.join(format!("{}{}.json", language.slug(), size.suffix()));
    if let Some(words) = read_wordlist(&path, size) {
        return Ok(words);
    }

    let url = format!(
        "https://raw.githubusercontent.com/monkeytypegame/monkeytype/{MONKEYTYPE_COMMIT}/frontend/static/languages/{}{}.json",
        language.slug(),
        size.suffix()
    );
    let text = get(&url)?;
    let list: MonkeytypeList =
        serde_json::from_str(&text).context("invalid Monkeytype wordlist")?;
    if !size.expected_range().contains(&list.words.len()) {
        bail!("wordlist has unexpected {} word count", list.words.len());
    }
    atomic_write(&path, text.as_bytes())?;
    Ok(list.words)
}

fn read_wordlist(path: &Path, size: WordListSize) -> Option<Vec<String>> {
    let text = fs::read_to_string(path).ok()?;
    let list = serde_json::from_str::<MonkeytypeList>(&text).ok()?;
    size.expected_range()
        .contains(&list.words.len())
        .then_some(list.words)
}

fn read_snippets(language: CodeLanguage) -> Result<Vec<Snippet>> {
    let directory = snippet_directory(language);
    if !directory.exists() {
        return Ok(Vec::new());
    }
    let mut snippets = Vec::new();
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if !path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            continue;
        }
        let Ok(text) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(mut snippet) = serde_json::from_str::<Snippet>(&text) else {
            continue;
        };
        let Some(without_comments) = strip_comments(snippet.language, &snippet.text) else {
            continue;
        };
        let source_indent = if snippet.indent_normalized {
            0
        } else {
            legacy_indent(&without_comments, snippet.language)
        };
        let Some(normalized) = normalize_snippet(&without_comments, source_indent) else {
            fs::remove_file(path)?;
            continue;
        };
        let changed = snippet.text != normalized || !snippet.indent_normalized;
        snippet.text = normalized;
        snippet.indent_normalized = true;
        if changed {
            atomic_write(&path, &serde_json::to_vec(&snippet)?)?;
        }
        snippets.push(snippet);
    }
    Ok(snippets)
}

fn refresh_snippets(language: CodeLanguage) -> Result<Vec<Snippet>> {
    let mut processed = read_processed(language);
    let mut repositories = search_repositories(language)?;
    repositories.retain(|repository| {
        !processed.names.contains(&repository.full_name)
            && repository.license.as_ref().is_some_and(|license| {
                ALLOWED_LICENSES.contains(&license.spdx_id.to_lowercase().as_str())
            })
    });
    repositories.shuffle(&mut rand::thread_rng());
    let directory = snippet_directory(language);
    fs::create_dir_all(&directory)?;
    let mut added = 0;
    for repository in repositories.into_iter().take(5) {
        let snippets = snippets_from_repository(language, &repository)?;
        processed.names.insert(repository.full_name);
        write_processed(language, &processed)?;
        for snippet in &snippets {
            atomic_write(
                &directory.join(format!("{}.json", snippet.id)),
                serde_json::to_vec(snippet)?.as_slice(),
            )?;
        }
        added += snippets.len();
        if added >= 20 {
            break;
        }
    }
    if added == 0 {
        bail!("no suitable complete structures found; cached snippets remain available");
    }
    read_snippets(language)
}

fn search_repositories(language: CodeLanguage) -> Result<Vec<Repository>> {
    let cache = cache_root()
        .join("github")
        .join(format!("search-{}.json", language.slug()));
    if cache_is_fresh(&cache, 6 * 60 * 60)
        && let Ok(body) = fs::read_to_string(&cache)
        && let Ok(response) = serde_json::from_str::<SearchResponse>(&body)
    {
        return Ok(response.items);
    }
    let query = format!(
        "language:{} stars:>=500 archived:false fork:false size:<200000",
        language.github_name()
    );
    let mut request = ureq::get("https://api.github.com/search/repositories")
        .query("q", &query)
        .query("sort", "updated")
        .query("order", "desc")
        .query("per_page", "50")
        .header("User-Agent", "typeforge")
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let body = match request.call() {
        Ok(mut response) => {
            let body = response.body_mut().read_to_string()?;
            atomic_write(&cache, body.as_bytes())?;
            body
        }
        Err(error) => fs::read_to_string(&cache)
            .with_context(|| format!("GitHub repository search failed: {error}"))?,
    };
    Ok(serde_json::from_str::<SearchResponse>(&body)?.items)
}

fn snippets_from_repository(
    language: CodeLanguage,
    repository: &Repository,
) -> Result<Vec<Snippet>> {
    let tree_url = format!(
        "https://api.github.com/repos/{}/git/trees/{}?recursive=1",
        repository.full_name, repository.default_branch
    );
    let tree_cache = cache_root()
        .join("github")
        .join(format!("tree-{}.json", hash(&repository.full_name)));
    let tree: TreeResponse = serde_json::from_str(&cached_github_get(&tree_url, &tree_cache)?)?;
    let mut files: Vec<_> = tree
        .tree
        .into_iter()
        .filter(|entry| {
            entry.kind == "blob"
                && entry
                    .size
                    .is_some_and(|size| (100..=150_000).contains(&size))
                && Path::new(&entry.path)
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| language.extensions().contains(&extension))
                && !entry.path.contains("vendor/")
                && !entry.path.contains("generated/")
                && !entry.path.contains("node_modules/")
        })
        .collect();
    files.shuffle(&mut rand::thread_rng());
    let mut snippets = Vec::new();
    for entry in files.into_iter().take(20) {
        let raw_url = format!(
            "https://raw.githubusercontent.com/{}/{}/{}",
            repository.full_name, repository.default_branch, entry.path
        );
        let Ok(source) = get(&raw_url) else {
            continue;
        };
        let license = repository
            .license
            .as_ref()
            .map(|license| license.spdx_id.clone())
            .unwrap_or_default();
        snippets.extend(extract_snippets(
            language,
            &source,
            repository,
            &entry.path,
            &license,
        ));
        if snippets.len() >= 50 {
            break;
        }
    }
    snippets.truncate(80);
    Ok(snippets)
}

fn extract_snippets(
    language: CodeLanguage,
    source: &str,
    repository: &Repository,
    path: &str,
    license: &str,
) -> Vec<Snippet> {
    let mut parser = Parser::new();
    if parser
        .set_language(&tree_sitter_language(language))
        .is_err()
    {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };
    let mut nodes = Vec::new();
    collect_nodes(tree.root_node(), language, &mut nodes);
    nodes
        .into_iter()
        .filter_map(|node| {
            let raw = node.utf8_text(source.as_bytes()).ok()?;
            let source_indent = visual_column(source, node.start_byte());
            let without_comments = strip_comments(language, raw)?;
            let text = normalize_snippet(&without_comments, source_indent)?;
            let id = hash(&format!(
                "{}:{path}:{}:{text}",
                repository.full_name,
                node.start_byte()
            ));
            Some(Snippet {
                id,
                language,
                kind: node.kind().replace('_', " "),
                text,
                repository: repository.full_name.clone(),
                path: path.into(),
                url: format!(
                    "{}/blob/{}/{}",
                    repository.html_url, repository.default_branch, path
                ),
                license: license.into(),
                indent_normalized: true,
            })
        })
        .collect()
}

fn collect_nodes<'a>(node: Node<'a>, language: CodeLanguage, output: &mut Vec<Node<'a>>) {
    if candidate_kinds(language).contains(&node.kind()) {
        output.push(node);
    }
    let mut cursor = node.walk();
    for child in node.named_children(&mut cursor) {
        collect_nodes(child, language, output);
    }
}

fn strip_comments(language: CodeLanguage, text: &str) -> Option<String> {
    let mut parser = Parser::new();
    parser.set_language(&tree_sitter_language(language)).ok()?;
    let tree = parser.parse(text, None)?;
    let mut ranges = Vec::new();
    collect_comment_ranges(tree.root_node(), &mut ranges);
    let mut output = text.to_owned();
    for range in ranges.into_iter().rev() {
        let newlines = output[range.clone()].matches('\n').count();
        output.replace_range(range, &"\n".repeat(newlines));
    }
    Some(
        output
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn collect_comment_ranges(node: Node<'_>, output: &mut Vec<std::ops::Range<usize>>) {
    if node.kind().contains("comment") {
        output.push(node.byte_range());
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_comment_ranges(child, output);
    }
}

fn candidate_kinds(language: CodeLanguage) -> &'static [&'static str] {
    match language {
        CodeLanguage::Python => &["function_definition", "class_definition"],
        CodeLanguage::JavaScript | CodeLanguage::TypeScript => &[
            "function_declaration",
            "class_declaration",
            "method_definition",
            "interface_declaration",
        ],
        CodeLanguage::Rust => &[
            "function_item",
            "impl_item",
            "struct_item",
            "enum_item",
            "trait_item",
        ],
        CodeLanguage::Go => &[
            "function_declaration",
            "method_declaration",
            "type_declaration",
        ],
        CodeLanguage::Swift => &[
            "function_declaration",
            "class_declaration",
            "struct_declaration",
            "protocol_declaration",
        ],
        CodeLanguage::Kotlin => &[
            "function_declaration",
            "class_declaration",
            "object_declaration",
        ],
        CodeLanguage::Cpp => &["function_definition", "class_specifier", "struct_specifier"],
        CodeLanguage::CSharp => &[
            "method_declaration",
            "class_declaration",
            "struct_declaration",
            "interface_declaration",
        ],
        CodeLanguage::Java => &[
            "method_declaration",
            "class_declaration",
            "interface_declaration",
            "record_declaration",
        ],
    }
}

fn normalize_snippet(raw: &str, source_indent: usize) -> Option<String> {
    let expanded = expand_tabs(raw);
    let raw = expanded.trim_matches('\n');
    let lines: Vec<String> = raw
        .lines()
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                line.to_owned()
            } else {
                line.strip_prefix(&" ".repeat(source_indent))
                    .unwrap_or(line)
                    .to_owned()
            }
        })
        .collect();
    if !(3..=20).contains(&lines.len()) || !(100..=600).contains(&raw.chars().count()) {
        return None;
    }
    let indentation = lines
        .iter()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .min()
        .unwrap_or(0);
    let text = lines
        .iter()
        .map(|line| line.get(indentation..).unwrap_or(line).trim_end())
        .collect::<Vec<_>>()
        .join("\n");
    let positive_indentation = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.len() - line.trim_start().len())
        .filter(|indentation| *indentation > 0)
        .min();
    if positive_indentation.is_none_or(|indentation| indentation < 2)
        || text.lines().any(|line| line.chars().count() > 88)
        || text
            .chars()
            .any(|character| !character.is_ascii() || character == '\0' || character == '\r')
    {
        return None;
    }
    Some(text)
}

fn visual_column(source: &str, start_byte: usize) -> usize {
    source[..start_byte]
        .rsplit_once('\n')
        .map_or(&source[..start_byte], |(_, line)| line)
        .chars()
        .fold(0, |column, character| {
            if character == '\t' {
                column + (4 - column % 4)
            } else {
                column + 1
            }
        })
}

fn legacy_indent(text: &str, language: CodeLanguage) -> usize {
    let lines: Vec<_> = text.lines().collect();
    match language {
        CodeLanguage::Python => lines
            .iter()
            .skip(1)
            .filter(|line| !line.trim().is_empty())
            .map(|line| line.len() - line.trim_start().len())
            .min()
            .unwrap_or(4)
            .saturating_sub(4),
        _ => lines
            .iter()
            .rev()
            .find(|line| !line.trim().is_empty())
            .filter(|line| matches!(line.trim(), "}" | "};" | "},"))
            .map(|line| line.len() - line.trim_start().len())
            .unwrap_or(0),
    }
}

fn refresh_in_background_if_due(language: CodeLanguage) {
    let stamp = cache_root()
        .join("indexes")
        .join(format!("{}-refresh", language.slug()));
    if cache_is_fresh(&stamp, 5 * 60) || atomic_write(&stamp, b"").is_err() {
        return;
    }
    std::thread::spawn(move || {
        let _ = refresh_snippets(language);
        let _ = enforce_cache_limit();
    });
}

fn expand_tabs(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut column = 0;
    for character in text.chars() {
        match character {
            '\n' => {
                output.push(character);
                column = 0;
            }
            '\t' => {
                let spaces = 4 - column % 4;
                output.extend(std::iter::repeat_n(' ', spaces));
                column += spaces;
            }
            _ => {
                output.push(character);
                column += 1;
            }
        }
    }
    output
}

fn tree_sitter_language(language: CodeLanguage) -> Language {
    match language {
        CodeLanguage::Python => tree_sitter_python::LANGUAGE.into(),
        CodeLanguage::JavaScript => tree_sitter_javascript::LANGUAGE.into(),
        CodeLanguage::TypeScript => tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        CodeLanguage::Rust => tree_sitter_rust::LANGUAGE.into(),
        CodeLanguage::Go => tree_sitter_go::LANGUAGE.into(),
        CodeLanguage::Swift => tree_sitter_swift::LANGUAGE.into(),
        CodeLanguage::Kotlin => tree_sitter_kotlin_ng::LANGUAGE.into(),
        CodeLanguage::Cpp => tree_sitter_cpp::LANGUAGE.into(),
        CodeLanguage::CSharp => tree_sitter_c_sharp::LANGUAGE.into(),
        CodeLanguage::Java => tree_sitter_java::LANGUAGE.into(),
    }
}

fn github_get(url: &str) -> Result<String> {
    let mut request = ureq::get(url)
        .header("User-Agent", "typeforge")
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28");
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        request = request.header("Authorization", format!("Bearer {token}"));
    }
    let mut response = request.call().context("GitHub request failed")?;
    response.body_mut().read_to_string().map_err(Into::into)
}

fn cached_github_get(url: &str, path: &Path) -> Result<String> {
    if let Ok(body) = fs::read_to_string(path) {
        return Ok(body);
    }
    let body = github_get(url)?;
    atomic_write(path, body.as_bytes())?;
    Ok(body)
}

fn get(url: &str) -> Result<String> {
    let mut response = ureq::get(url)
        .header("User-Agent", "typeforge")
        .call()
        .context("dataset request failed")?;
    response.body_mut().read_to_string().map_err(Into::into)
}

fn snippet_directory(language: CodeLanguage) -> PathBuf {
    cache_root().join("snippets").join(language.slug())
}

fn processed_path(language: CodeLanguage) -> PathBuf {
    cache_root()
        .join("indexes")
        .join(format!("{}-repositories.json", language.slug()))
}

fn read_processed(language: CodeLanguage) -> ProcessedRepos {
    fs::read_to_string(processed_path(language))
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

fn write_processed(language: CodeLanguage, value: &ProcessedRepos) -> Result<()> {
    atomic_write(&processed_path(language), &serde_json::to_vec(value)?)
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("cache path has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = path.with_extension("tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(temporary, path)?;
    Ok(())
}

fn hash(value: &str) -> String {
    format!("{:x}", Sha256::digest(value.as_bytes()))
}

fn cache_is_fresh(path: &Path, maximum_age_secs: u64) -> bool {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.elapsed().ok())
        .is_some_and(|age| age.as_secs() <= maximum_age_secs)
}

fn enforce_cache_limit() -> Result<()> {
    enforce_cache_limit_at(&cache_root(), CACHE_LIMIT)
}

fn enforce_cache_limit_at(root: &Path, limit: u64) -> Result<()> {
    if !root.exists() {
        return Ok(());
    }
    let mut files = Vec::new();
    collect_files(root, &mut files)?;
    let mut total: u64 = files.iter().map(|(_, size)| size).sum();
    if total <= limit {
        return Ok(());
    }
    files.sort_by_key(|(path, _)| {
        fs::metadata(path)
            .and_then(|metadata| metadata.modified())
            .ok()
    });
    for (path, size) in files {
        if total <= limit {
            break;
        }
        if path.starts_with(root.join("snippets")) || path.starts_with(root.join("github")) {
            fs::remove_file(path)?;
            total = total.saturating_sub(size);
        }
    }
    Ok(())
}

fn collect_files(directory: &Path, files: &mut Vec<(PathBuf, u64)>) -> Result<()> {
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_files(&path, files)?;
        } else {
            files.push((path.clone(), fs::metadata(path)?.len()));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn temporary_directory() -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path =
            std::env::temp_dir().join(format!("typeforge-corpus-{}-{suffix}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn snippet_limits_are_enforced() {
        assert!(normalize_snippet("fn calculate_total() {\n    let subtotal = 12345678901234567890123456789012345678901234567890;\n    let tax = subtotal / 5;\n    subtotal + tax\n}", 0).is_some());
        assert!(normalize_snippet("fn a() {}\n", 0).is_none());
    }

    #[test]
    fn nested_structure_indentation_becomes_relative() {
        let raw = "async isWaiting(): Promise<boolean> {\n        return this.waiting || this.paused;\n    }";
        let padded =
            format!("{raw}\n    void this_line_makes_the_snippet_long_enough_to_validate();");
        let normalized = normalize_snippet(&padded, 4).unwrap();
        assert!(normalized.contains("\n    return"));
        assert!(!normalized.contains("\n        return"));
    }

    #[test]
    fn tabs_expand_to_visual_stops() {
        assert_eq!(expand_tabs("  \tvalue\n\tvalue"), "    value\n    value");
    }

    #[test]
    fn malformed_one_space_indentation_is_rejected() {
        let source = "class ImplicitStage implements MarkStage {\n  constructor(private ide: IDE) {}\n  run(): Target[] {\n    return getActiveSelections(this.ide).map(\n      (selection) =>\n new ImplicitTarget({\n   editor: selection.editor,\n   isReversed: selection.selection.isReversed,\n   contentRange: selection.selection,\n }),\n    );\n  }\n}";
        assert!(normalize_snippet(source, 0).is_none());
    }

    #[test]
    fn code_snippets_reject_non_ascii_input() {
        let source = "fn panic_with_emoji() {\n    let message = \"bad 😂 input that cannot be typed normally\";\n    panic!(\"{message}\");\n}";
        assert!(normalize_snippet(source, 0).is_none());
    }

    #[test]
    fn comments_are_removed_without_touching_strings() {
        let source = "fn url() {\n    // irrelevant explanation\n    let url = \"https://example.com\"; /* noise */\n    println!(\"{url}\");\n}";
        let stripped = strip_comments(CodeLanguage::Rust, source).unwrap();
        assert!(!stripped.contains("irrelevant"));
        assert!(!stripped.contains("noise"));
        assert!(stripped.contains("https://example.com"));
        assert_eq!(stripped.lines().count(), 4);
    }

    #[test]
    fn every_language_finds_complete_structures() {
        let cases = [
            (CodeLanguage::Python, "def greet():\n    return 'hello'\n"),
            (
                CodeLanguage::JavaScript,
                "function greet() { return 'hello'; }",
            ),
            (CodeLanguage::TypeScript, "interface User { name: string; }"),
            (
                CodeLanguage::Rust,
                "fn greet() -> &'static str { \"hello\" }",
            ),
            (CodeLanguage::Go, "func greet() string { return \"hello\" }"),
            (
                CodeLanguage::Swift,
                "func greet() -> String { return \"hello\" }",
            ),
            (
                CodeLanguage::Kotlin,
                "fun greet(): String { return \"hello\" }",
            ),
            (CodeLanguage::Cpp, "int greet() { return 1; }"),
            (
                CodeLanguage::CSharp,
                "class Greeter { int Greet() { return 1; } }",
            ),
            (
                CodeLanguage::Java,
                "class Greeter { int greet() { return 1; } }",
            ),
        ];
        for (language, source) in cases {
            let mut parser = Parser::new();
            parser
                .set_language(&tree_sitter_language(language))
                .unwrap();
            let tree = parser.parse(source, None).unwrap();
            let mut nodes = Vec::new();
            collect_nodes(tree.root_node(), language, &mut nodes);
            assert!(!nodes.is_empty(), "{}", language.label());
        }
    }

    #[test]
    fn cached_wordlists_must_be_valid() {
        let directory = temporary_directory();
        let path = directory.join("english.json");
        let words = vec!["word"; 200];
        fs::write(
            &path,
            serde_json::to_vec(&serde_json::json!({ "words": words })).unwrap(),
        )
        .unwrap();
        assert_eq!(
            read_wordlist(&path, WordListSize::Top200).unwrap().len(),
            200
        );

        fs::write(&path, b"invalid").unwrap();
        assert!(read_wordlist(&path, WordListSize::Top200).is_none());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn cache_limit_preserves_wordlists() {
        let directory = temporary_directory();
        let wordlist = directory.join("wordlists/english.json");
        let github = directory.join("github/search.json");
        atomic_write(&wordlist, b"12345678").unwrap();
        atomic_write(&github, b"12345678").unwrap();

        enforce_cache_limit_at(&directory, 8).unwrap();
        assert!(wordlist.exists());
        assert!(!github.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    #[ignore]
    fn pinned_wordlist_is_downloadable() {
        let words = load_words(WordLanguage::English, WordListSize::Top200).unwrap();
        assert_eq!(words.len(), 200);
    }

    #[test]
    #[ignore]
    fn rust_snippet_pool_is_downloadable() {
        let snippets = refresh_snippets(CodeLanguage::Rust).unwrap();
        assert!(!snippets.is_empty());
    }

    #[test]
    #[ignore]
    fn cached_snippets_are_migrated() {
        for language in CodeLanguage::ALL {
            read_snippets(language).unwrap();
        }
    }
}
