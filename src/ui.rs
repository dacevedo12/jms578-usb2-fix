//! Terminal interaction, plus a scripted implementation for tests.

use std::collections::VecDeque;
use std::io::{BufRead, IsTerminal, Write};

pub trait Ui {
    fn step(&mut self, n: usize, total: usize, title: &str);
    fn say(&mut self, text: &str);
    fn good(&mut self, text: &str);
    fn warn(&mut self, text: &str);
    fn bad(&mut self, text: &str);
    /// Waits for Enter.
    fn pause(&mut self, prompt: &str);
    /// Yes/no question, defaulting to no.
    fn confirm(&mut self, prompt: &str) -> bool;
    /// Requires typing `word` exactly, for irreversible-feeling actions.
    fn confirm_word(&mut self, prompt: &str, word: &str) -> bool;
    fn choose(&mut self, prompt: &str, options: &[String]) -> Option<usize>;
    fn progress(&mut self, label: &str, fraction: f64);
    /// Sleeps between hardware polls (instant in tests).
    fn wait(&mut self, millis: u64);
}

pub struct Terminal {
    color: bool,
    interactive: bool,
    last_percent: Option<u32>,
}

impl Default for Terminal {
    fn default() -> Self {
        let interactive = std::io::stdout().is_terminal();
        Self {
            color: interactive && std::env::var_os("NO_COLOR").is_none(),
            interactive,
            last_percent: None,
        }
    }
}

impl Terminal {
    fn paint(&self, code: &str, text: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{text}\x1b[0m")
        } else {
            text.to_string()
        }
    }

    fn read_line(prompt: &str) -> String {
        print!("{prompt}");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        let _ = std::io::stdin().lock().read_line(&mut line);
        line.trim().to_string()
    }
}

impl Ui for Terminal {
    fn step(&mut self, n: usize, total: usize, title: &str) {
        println!("\n{}", self.paint("1;36", &format!("Step {n}/{total}: {title}")));
    }
    fn say(&mut self, text: &str) {
        println!("  {}", text.replace('\n', "\n  "));
    }
    fn good(&mut self, text: &str) {
        println!("  {} {text}", self.paint("32", "✓"));
    }
    fn warn(&mut self, text: &str) {
        println!("  {} {}", self.paint("33", "!"), text.replace('\n', "\n    "));
    }
    fn bad(&mut self, text: &str) {
        println!("  {} {}", self.paint("31", "✗"), text.replace('\n', "\n    "));
    }
    fn pause(&mut self, prompt: &str) {
        Self::read_line(&format!("  {prompt} [press Enter] "));
    }
    fn confirm(&mut self, prompt: &str) -> bool {
        matches!(
            Self::read_line(&format!("  {prompt} [y/N] "))
                .to_lowercase()
                .as_str(),
            "y" | "yes"
        )
    }
    fn confirm_word(&mut self, prompt: &str, word: &str) -> bool {
        Self::read_line(&format!("  {prompt} Type \"{word}\" to continue: ")) == word
    }
    fn choose(&mut self, prompt: &str, options: &[String]) -> Option<usize> {
        println!("  {prompt}");
        for (i, o) in options.iter().enumerate() {
            println!("    {}) {o}", i + 1);
        }
        let answer = Self::read_line("  Number (or Enter to cancel): ");
        answer
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=options.len()).contains(n))
            .map(|n| n - 1)
    }
    fn progress(&mut self, label: &str, fraction: f64) {
        let percent = (fraction.clamp(0.0, 1.0) * 100.0).floor() as u32;
        // A lower value than last time means a new bar; repeats are skipped.
        if self.last_percent.is_some_and(|last| percent == last) {
            return;
        }
        self.last_percent = Some(percent);
        if !self.interactive && percent != 100 {
            return;
        }
        let width = 30;
        let filled = (percent * width / 100) as usize;
        print!(
            "\r  {label} [{}{}] {percent:>3}%",
            "#".repeat(filled),
            " ".repeat(width as usize - filled)
        );
        if percent == 100 {
            println!();
        }
        let _ = std::io::stdout().flush();
    }
    fn wait(&mut self, millis: u64) {
        std::thread::sleep(std::time::Duration::from_millis(millis));
    }
}

/// Answers prompts from a script and records everything shown, for tests.
#[derive(Default)]
pub struct Scripted {
    pub answers: VecDeque<String>,
    pub transcript: Vec<String>,
    /// Called on every pause, so tests can act like the user (e.g. replug the adapter).
    pub on_pause: Vec<String>,
}

impl Scripted {
    #[must_use]
    pub fn new(answers: &[&str]) -> Self {
        Self {
            answers: answers.iter().map(ToString::to_string).collect(),
            ..Self::default()
        }
    }

    fn answer(&mut self) -> String {
        self.answers.pop_front().unwrap_or_default()
    }

    #[must_use]
    pub fn output(&self) -> String {
        self.transcript.join("\n")
    }
}

impl Ui for Scripted {
    fn step(&mut self, n: usize, total: usize, title: &str) {
        self.transcript.push(format!("Step {n}/{total}: {title}"));
    }
    fn say(&mut self, text: &str) {
        self.transcript.push(text.to_string());
    }
    fn good(&mut self, text: &str) {
        self.transcript.push(format!("✓ {text}"));
    }
    fn warn(&mut self, text: &str) {
        self.transcript.push(format!("! {text}"));
    }
    fn bad(&mut self, text: &str) {
        self.transcript.push(format!("✗ {text}"));
    }
    fn pause(&mut self, prompt: &str) {
        self.transcript.push(format!("[pause] {prompt}"));
        self.on_pause.push(prompt.to_string());
    }
    fn confirm(&mut self, prompt: &str) -> bool {
        self.transcript.push(format!("[confirm] {prompt}"));
        matches!(self.answer().as_str(), "y" | "yes")
    }
    fn confirm_word(&mut self, prompt: &str, word: &str) -> bool {
        self.transcript.push(format!("[type {word}] {prompt}"));
        self.answer() == word
    }
    fn choose(&mut self, prompt: &str, options: &[String]) -> Option<usize> {
        self.transcript
            .push(format!("[choose] {prompt}: {}", options.join(" | ")));
        self.answer()
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=options.len()).contains(n))
            .map(|n| n - 1)
    }
    fn progress(&mut self, _label: &str, _fraction: f64) {}
    fn wait(&mut self, _millis: u64) {}
}
