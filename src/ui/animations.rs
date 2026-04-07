use crossterm::{
    cursor, execute,
    style::{self, Color, Print, StyledContent, Stylize},
    terminal::{self, ClearType},
};
use std::io::{stdout, Write};
use std::time::Duration;

pub struct AnimationConfig {
    pub typewriter_delay_ms: u64,
    pub show_skip_prompt: bool,
    pub colors: AnimationColors,
}

impl Default for AnimationConfig {
    fn default() -> Self {
        Self {
            typewriter_delay_ms: 8,
            show_skip_prompt: true,
            colors: AnimationColors::default(),
        }
    }
}

pub struct AnimationColors {
    pub primary: style::Color,
    pub secondary: style::Color,
    pub accent: style::Color,
    pub dim: style::Color,
    pub background: style::Color,
}

impl Default for AnimationColors {
    fn default() -> Self {
        Self {
            primary: Color::Cyan,
            secondary: Color::Blue,
            accent: Color::Green,
            dim: Color::DarkGrey,
            background: Color::Black,
        }
    }
}

fn get_terminal_size() -> (u16, u16) {
    terminal::size().unwrap_or((80, 24))
}

fn center_x(text: &str, width: u16) -> u16 {
    let text_width = text.len() as u16;
    if text_width >= width {
        0
    } else {
        (width - text_width) / 2
    }
}

pub fn play_startup_animation(config: &AnimationConfig) -> bool {
    let (cols, rows) = get_terminal_size();
    if cols < 60 || rows < 20 {
        return false;
    }

    let mut stdout = stdout();
    let _ = execute!(stdout, terminal::EnterAlternateScreen);
    let _ = execute!(stdout, cursor::Hide);
    let _ = execute!(stdout, style::SetBackgroundColor(config.colors.background));
    let _ = execute!(stdout, terminal::Clear(ClearType::All));
    let _ = stdout.flush();

    let logo_lines = get_logo_art();
    let subtitle = "  Secure • Encrypted • Real-time Chat  ";
    let version = "v0.3.0";

    let box_height = 22u16;
    let start_y = (rows - box_height) / 2;
    let start_x = center_x(
        "╔════════════════════════════════════════════════════════════╗",
        cols,
    );

    for (i, line) in logo_lines.iter().enumerate() {
        let y = start_y + 2 + i as u16;
        let _ = execute!(stdout, cursor::MoveTo(start_x, y));

        let padded_line = if line.len() < 64 {
            format!("{:^64}", line)
        } else {
            line.chars().take(64).collect()
        };

        let colored: StyledContent<String> = padded_line.with(config.colors.primary);
        execute!(stdout, Print(colored)).ok();
        let _ = stdout.flush();
        std::thread::sleep(Duration::from_millis(15));
    }

    let subtitle_y = start_y + 2 + logo_lines.len() as u16 + 1;
    let sub_x = center_x(subtitle, cols);

    for ch in subtitle.chars() {
        let _ = execute!(
            stdout,
            cursor::MoveTo(sub_x, subtitle_y),
            Print(ch.to_string().with(config.colors.secondary))
        );
        let _ = stdout.flush();
        std::thread::sleep(Duration::from_millis(30));
    }

    let version_y = subtitle_y + 2;
    let version_text = format!("  {}  ", version);
    let ver_x = center_x(&version_text, cols);
    let _ = execute!(
        stdout,
        cursor::MoveTo(ver_x, version_y),
        Print(version_text.with(config.colors.dim))
    );

    if config.show_skip_prompt {
        let prompt = "  Press any key to continue...  ";
        let prompt_y = start_y + box_height - 2;
        let prompt_x = center_x(prompt, cols);

        for i in 0..3 {
            let _ = execute!(
                stdout,
                cursor::MoveTo(prompt_x, prompt_y),
                terminal::Clear(ClearType::UntilNewLine)
            );

            let visible_chars = (prompt.len() as f32 * (i as f32 / 3.0)) as usize;
            let partial = &prompt[..visible_chars.min(prompt.len())];

            if i == 2 {
                let _ = execute!(
                    stdout,
                    cursor::MoveTo(prompt_x, prompt_y),
                    Print(prompt.with(config.colors.dim))
                );
            } else {
                let _ = execute!(
                    stdout,
                    cursor::MoveTo(prompt_x, prompt_y),
                    Print(partial.with(config.colors.dim))
                );
            }
            let _ = stdout.flush();
            std::thread::sleep(Duration::from_millis(200));
        }

        let blink_text = "Press any key to continue...".to_string();
        for _ in 0..6 {
            let _ = execute!(
                stdout,
                cursor::MoveTo(prompt_x, prompt_y),
                terminal::Clear(ClearType::UntilNewLine)
            );
            let _ = execute!(
                stdout,
                cursor::MoveTo(prompt_x, prompt_y),
                Print(blink_text.clone().with(config.colors.dim))
            );
            let _ = stdout.flush();
            std::thread::sleep(Duration::from_millis(250));

            let _ = execute!(
                stdout,
                cursor::MoveTo(prompt_x, prompt_y),
                terminal::Clear(ClearType::UntilNewLine)
            );
            let _ = stdout.flush();
            std::thread::sleep(Duration::from_millis(250));
        }

        let _ = execute!(
            stdout,
            cursor::MoveTo(prompt_x, prompt_y),
            terminal::Clear(ClearType::UntilNewLine)
        );
        let _ = execute!(
            stdout,
            cursor::MoveTo(prompt_x, prompt_y),
            Print("Press any key to continue...".with(config.colors.accent))
        );
    }

    let _ = stdout.flush();

    if config.show_skip_prompt {
        if crossterm::event::poll(Duration::from_millis(50)).unwrap_or(false) {
            let _ = execute!(stdout, cursor::Show);
            let _ = execute!(stdout, terminal::LeaveAlternateScreen);
            let _ = execute!(stdout, terminal::Clear(ClearType::All));
            let _ = stdout.flush();
            return true;
        }
    }

    false
}

pub fn play_exit_animation(config: &AnimationConfig) {
    let (cols, rows) = get_terminal_size();
    if cols < 50 || rows < 10 {
        return;
    }

    let mut stdout = stdout();

    let _box_width = 50u16;
    let box_height = 10u16;
    let start_y = (rows - box_height) / 2;
    let _start_x = center_x("╔════════════════════════════════════════════════╗", cols);

    for i in 0..=box_height {
        let current_y = start_y + i;

        if i == 0 {
            let top = "╔════════════════════════════════════════════╗".to_string();
            let x = center_x(&top, cols);
            let _ = execute!(stdout, cursor::MoveTo(x, current_y));
            let _ = execute!(stdout, Print(top.with(config.colors.primary)));
        } else if i == box_height {
            let bottom = "╚════════════════════════════════════════════╝".to_string();
            let x = center_x(&bottom, cols);
            let _ = execute!(stdout, cursor::MoveTo(x, current_y));
            let _ = execute!(stdout, Print(bottom.with(config.colors.primary)));
        } else {
            let spaces = " ".repeat(48);
            let x = center_x(&format!("║{}║", spaces), cols);
            let _ = execute!(stdout, cursor::MoveTo(x, current_y));
            let _ = execute!(stdout, Print("║".to_string().with(config.colors.primary)));
            let _ = execute!(stdout, cursor::MoveRight(48));
            let _ = execute!(stdout, Print("║".to_string().with(config.colors.primary)));
        }
        let _ = stdout.flush();
        std::thread::sleep(Duration::from_millis(20));
    }

    let lines = vec![
        ("".to_string(), config.colors.primary),
        ("              Goodbye!".to_string(), config.colors.accent),
        ("".to_string(), config.colors.primary),
        (
            "      Thanks for using Chatify.".to_string(),
            config.colors.secondary,
        ),
        (
            "      Stay secure. Stay connected.".to_string(),
            config.colors.secondary,
        ),
        ("".to_string(), config.colors.primary),
        (
            "        (Press any key to exit)".to_string(),
            config.colors.dim,
        ),
        ("".to_string(), config.colors.primary),
    ];

    for (i, (line, color)) in lines.iter().enumerate() {
        let y = start_y + 1 + i as u16;
        let x = center_x(line, cols);

        for ch in line.chars() {
            let _ = execute!(stdout, cursor::MoveTo(x, y));
            let _ = execute!(stdout, Print(ch.to_string().with(*color)));
            let _ = stdout.flush();
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    let _ = stdout.flush();

    let prompt = "(Press any key to exit)".to_string();
    let prompt_y = start_y + box_height - 1;
    let prompt_x = center_x(&prompt, cols);

    for _ in 0..4 {
        let _ = execute!(
            stdout,
            cursor::MoveTo(prompt_x, prompt_y),
            terminal::Clear(ClearType::UntilNewLine)
        );
        let _ = execute!(
            stdout,
            cursor::MoveTo(prompt_x, prompt_y),
            Print(prompt.clone().with(config.colors.dim))
        );
        let _ = stdout.flush();
        std::thread::sleep(Duration::from_millis(200));

        let _ = execute!(
            stdout,
            cursor::MoveTo(prompt_x, prompt_y),
            terminal::Clear(ClearType::UntilNewLine)
        );
        let _ = stdout.flush();
        std::thread::sleep(Duration::from_millis(200));
    }

    let _ = execute!(
        stdout,
        cursor::MoveTo(prompt_x, prompt_y),
        terminal::Clear(ClearType::UntilNewLine)
    );
    let _ = execute!(
        stdout,
        cursor::MoveTo(prompt_x, prompt_y),
        Print(prompt.with(config.colors.accent))
    );
    let _ = stdout.flush();

    std::thread::sleep(Duration::from_millis(500));

    if config.show_skip_prompt {
        if crossterm::event::poll(Duration::from_millis(100)).unwrap_or(false) {
            let _ = execute!(stdout, cursor::Show);
            let _ = execute!(stdout, terminal::LeaveAlternateScreen);
            let _ = execute!(stdout, terminal::Clear(ClearType::All));
            let _ = stdout.flush();
            return;
        }
    }

    std::thread::sleep(Duration::from_millis(2000));

    let _ = execute!(stdout, cursor::Show);
    let _ = execute!(stdout, terminal::LeaveAlternateScreen);
    let _ = execute!(stdout, terminal::Clear(ClearType::All));
    let _ = stdout.flush();
}

pub fn cleanup_animation_state() {
    let mut stdout = stdout();
    let _ = execute!(stdout, cursor::Show);
    let _ = execute!(stdout, terminal::LeaveAlternateScreen);
    let _ = execute!(stdout, terminal::Clear(ClearType::All));
    let _ = stdout.flush();
}

fn get_logo_art() -> Vec<String> {
    vec![
        "".to_string(),
        "   ██████╗ ██████╗  ██████╗ ███╗   ██╗██╗    ██╗ █████╗   ".to_string(),
        "   ██╔══██╗██╔══██╗██╔═══██╗████╗  ██║██║    ██║██╔══██╗ ".to_string(),
        "   ██████╔╝██████╔╝██║   ██║██╔██╗ ██║██║ █╗ ██║███████║ ".to_string(),
        "   ██╔══██╗██╔══██╗██║   ██║██║╚██╗██║██║███╗██║██╔══██║ ".to_string(),
        "   ██║  ██║██║  ██║╚██████╔╝██║ ╚████║╚███╔███╔╝██║  ██║ ".to_string(),
        "   ╚═╝  ╚═╝╚═╝  ╚═╝ ╚═════╝ ╚═╝  ╚═══╝ ╚══╝╚══╝ ╚═╝  ╚═╝ ".to_string(),
        "".to_string(),
        "                ██╗  ██╗ ██████╗ ██╗  ██╗██╗               ".to_string(),
        "                ██║  ██║██╔═████╗██║  ██║██║               ".to_string(),
        "                ███████║██║██╔██║███████║██║               ".to_string(),
        "                ╚════██║████╔╝██║╚════██║██║               ".to_string(),
        "                     ██║╚██████╔╝     ██║██║               ".to_string(),
        "                     ╚═╝ ╚═════╝      ╚═╝╚═╝               ".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = AnimationConfig::default();
        assert_eq!(config.typewriter_delay_ms, 8);
        assert!(config.show_skip_prompt);
    }

    #[test]
    fn test_default_colors() {
        let colors = AnimationColors::default();
        assert!(matches!(colors.primary, Color::Cyan));
    }

    #[test]
    fn test_get_terminal_size() {
        let (cols, rows) = get_terminal_size();
        assert!(cols > 0);
        assert!(rows > 0);
    }

    #[test]
    fn test_logo_art_not_empty() {
        let logo = get_logo_art();
        assert!(!logo.is_empty());
        assert!(logo.len() > 10);
    }
}
