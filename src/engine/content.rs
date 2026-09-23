use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PracticeKind {
    Words,
    Code,
}

impl PracticeKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Words => "Words",
            Self::Code => "Code",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WordLanguage {
    English,
    Russian,
}

impl WordLanguage {
    pub fn label(self) -> &'static str {
        match self {
            Self::English => "English",
            Self::Russian => "Russian",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::English => "english",
            Self::Russian => "russian",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WordListSize {
    Top200,
    Top1k,
    Top5k,
    Top10k,
}

impl WordListSize {
    pub fn label(self) -> &'static str {
        match self {
            Self::Top200 => "200",
            Self::Top1k => "1k",
            Self::Top5k => "5k",
            Self::Top10k => "10k",
        }
    }

    pub fn suffix(self) -> &'static str {
        match self {
            Self::Top200 => "",
            Self::Top1k => "_1k",
            Self::Top5k => "_5k",
            Self::Top10k => "_10k",
        }
    }

    pub fn expected_range(self) -> std::ops::RangeInclusive<usize> {
        match self {
            Self::Top200 => 190..=210,
            Self::Top1k => 950..=1_050,
            Self::Top5k => 4_700..=5_100,
            Self::Top10k => 9_400..=10_100,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WordLength {
    Any,
    Short,
    Medium,
    Long,
}

impl WordLength {
    pub fn label(self) -> &'static str {
        match self {
            Self::Any => "Any",
            Self::Short => "1-4",
            Self::Medium => "5-8",
            Self::Long => "9+",
        }
    }

    pub fn accepts(self, word: &str) -> bool {
        let len = word.chars().count();
        match self {
            Self::Any => true,
            Self::Short => len <= 4,
            Self::Medium => (5..=8).contains(&len),
            Self::Long => len >= 9,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CodeLanguage {
    Python,
    JavaScript,
    TypeScript,
    Rust,
    Go,
    Swift,
    Kotlin,
    Cpp,
    CSharp,
    Java,
}

impl CodeLanguage {
    pub const ALL: [Self; 10] = [
        Self::Python,
        Self::JavaScript,
        Self::TypeScript,
        Self::Rust,
        Self::Go,
        Self::Swift,
        Self::Kotlin,
        Self::Cpp,
        Self::CSharp,
        Self::Java,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Python => "Python",
            Self::JavaScript => "JavaScript",
            Self::TypeScript => "TypeScript",
            Self::Rust => "Rust",
            Self::Go => "Go",
            Self::Swift => "Swift",
            Self::Kotlin => "Kotlin",
            Self::Cpp => "C++",
            Self::CSharp => "C#",
            Self::Java => "Java",
        }
    }

    pub fn github_name(self) -> &'static str {
        match self {
            Self::Cpp => "C++",
            Self::CSharp => "C#",
            other => other.label(),
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            Self::JavaScript => "javascript",
            Self::TypeScript => "typescript",
            Self::CSharp => "csharp",
            Self::Cpp => "cpp",
            Self::Python => "python",
            Self::Rust => "rust",
            Self::Go => "go",
            Self::Swift => "swift",
            Self::Kotlin => "kotlin",
            Self::Java => "java",
        }
    }

    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Python => &["py"],
            Self::JavaScript => &["js", "jsx", "mjs"],
            Self::TypeScript => &["ts", "tsx"],
            Self::Rust => &["rs"],
            Self::Go => &["go"],
            Self::Swift => &["swift"],
            Self::Kotlin => &["kt", "kts"],
            Self::Cpp => &["cpp", "cc", "cxx", "hpp", "hxx"],
            Self::CSharp => &["cs"],
            Self::Java => &["java"],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MistakeMode {
    Strict,
    Free,
}

impl MistakeMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Strict => "Strict",
            Self::Free => "Free",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Snippet {
    pub id: String,
    pub language: CodeLanguage,
    pub kind: String,
    pub text: String,
    pub repository: String,
    pub path: String,
    pub url: String,
    pub license: String,
    #[serde(default)]
    pub indent_normalized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentSource {
    Words {
        language: WordLanguage,
        size: WordListSize,
    },
    Code(Snippet),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TypingContent {
    pub text: String,
    pub source: ContentSource,
}
