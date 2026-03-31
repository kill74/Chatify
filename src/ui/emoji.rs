//! Unicode emoji shortcode system for Chatify.
//!
//! Converts `:shortcode:` patterns in text to their Unicode emoji equivalents.
//! Provides search and listing capabilities for the `/emoji` command.

use std::collections::HashMap;
use std::sync::OnceLock;

type EmojiMap = HashMap<&'static str, &'static str>;

fn emoji_map() -> &'static EmojiMap {
    static MAP: OnceLock<EmojiMap> = OnceLock::new();
    MAP.get_or_init(|| {
        let mut m = HashMap::new();
        // Smileys & Emotion
        m.insert(":smile:", "😄");
        m.insert(":grinning:", "😀");
        m.insert(":laughing:", "😆");
        m.insert(":sweat_smile:", "😅");
        m.insert(":joy:", "😂");
        m.insert(":rofl:", "🤣");
        m.insert(":wink:", "😉");
        m.insert(":blush:", "😊");
        m.insert(":innocent:", "😇");
        m.insert(":slightly_smiling:", "🙂");
        m.insert(":upside_down:", "🙃");
        m.insert(":melting:", "🫠");
        m.insert(":yum:", "😋");
        m.insert(":sunglasses:", "😎");
        m.insert(":nerd:", "🤓");
        m.insert(":thinking:", "🤔");
        m.insert(":raised_eyebrow:", "🤨");
        m.insert(":neutral:", "😐");
        m.insert(":expressionless:", "😑");
        m.insert(":unamused:", "😒");
        m.insert(":roll_eyes:", "🙄");
        m.insert(":grimacing:", "😬");
        m.insert(":relieved:", "😌");
        m.insert(":pensive:", "😔");
        m.insert(":sleepy:", "😪");
        m.insert(":drooling:", "🤤");
        m.insert(":sleeping:", "😴");
        m.insert(":mask:", "😷");
        m.insert(":thermometer_face:", "🤒");
        m.insert(":head_bandage:", "🤕");
        m.insert(":nauseated:", "🤢");
        m.insert(":vomiting:", "🤮");
        m.insert(":sneezing:", "🤧");
        m.insert(":hot:", "🥵");
        m.insert(":cold:", "🥶");
        m.insert(":woozy:", "🥴");
        m.insert(":dizzy:", "😵");
        m.insert(":exploding:", "🤯");
        m.insert(":cowboy:", "🤠");
        m.insert(":partying:", "🥳");
        m.insert(":disguised:", "🥸");
        m.insert(":sad:", "😢");
        m.insert(":cry:", "😭");
        m.insert(":scream:", "😱");
        m.insert(":confounded:", "😖");
        m.insert(":persevere:", "😣");
        m.insert(":disappointed:", "😞");
        m.insert(":sweat:", "😓");
        m.insert(":weary:", "😩");
        m.insert(":tired:", "😫");
        m.insert(":yawning:", "🥱");
        m.insert(":triumph:", "😤");
        m.insert(":rage:", "😡");
        m.insert(":angry:", "😠");
        m.insert(":cursing:", "🤬");
        m.insert(":smiling_imp:", "😈");
        m.insert(":imp:", "👿");
        m.insert(":skull:", "💀");
        m.insert(":skull_crossbones:", "☠");
        m.insert(":poop:", "💩");
        m.insert(":clown:", "🤡");
        m.insert(":ghost:", "👻");
        m.insert(":alien:", "👽");
        m.insert(":robot:", "🤖");

        // Gestures & People
        m.insert(":wave:", "👋");
        m.insert(":raised_hand:", "✋");
        m.insert(":hand_splayed:", "🖐");
        m.insert(":vulcan:", "🖖");
        m.insert(":ok_hand:", "👌");
        m.insert(":pinched:", "🤌");
        m.insert(":pinch:", "🤏");
        m.insert(":v:", "✌");
        m.insert(":crossed_fingers:", "🤞");
        m.insert(":love_you:", "🤟");
        m.insert(":metal:", "🤘");
        m.insert(":call_me:", "🤙");
        m.insert(":point_left:", "👈");
        m.insert(":point_right:", "👉");
        m.insert(":point_up:", "👆");
        m.insert(":point_down:", "👇");
        m.insert(":thumbsup:", "👍");
        m.insert(":thumbsdown:", "👎");
        m.insert(":fist:", "✊");
        m.insert(":punch:", "👊");
        m.insert(":clap:", "👏");
        m.insert(":raised_hands:", "🙌");
        m.insert(":open_hands:", "👐");
        m.insert(":palms_up:", "🤲");
        m.insert(":handshake:", "🤝");
        m.insert(":pray:", "🙏");
        m.insert(":writing:", "✍");
        m.insert(":selfie:", "🤳");
        m.insert(":muscle:", "💪");
        m.insert(":leg:", "🦵");
        m.insert(":foot:", "🦶");
        m.insert(":ear:", "👂");
        m.insert(":nose:", "👃");
        m.insert(":brain:", "🧠");
        m.insert(":eyes:", "👀");
        m.insert(":eye:", "👁");
        m.insert(":tongue:", "👅");
        m.insert(":lips:", "👄");

        // Hearts & Love
        m.insert(":heart:", "❤");
        m.insert(":orange_heart:", "🧡");
        m.insert(":yellow_heart:", "💛");
        m.insert(":green_heart:", "💚");
        m.insert(":blue_heart:", "💙");
        m.insert(":purple_heart:", "💜");
        m.insert(":black_heart:", "🖤");
        m.insert(":white_heart:", "🤍");
        m.insert(":brown_heart:", "🤎");
        m.insert(":broken_heart:", "💔");
        m.insert(":heart_exclamation:", "❣");
        m.insert(":two_hearts:", "💕");
        m.insert(":revolving_hearts:", "💞");
        m.insert(":heartbeat:", "💓");
        m.insert(":heartpulse:", "💗");
        m.insert(":sparkling_heart:", "💖");
        m.insert(":cupid:", "💘");
        m.insert(":gift_heart:", "💝");
        m.insert(":kiss:", "💋");

        // Animals
        m.insert(":dog:", "🐶");
        m.insert(":cat:", "🐱");
        m.insert(":mouse:", "🐭");
        m.insert(":hamster:", "🐹");
        m.insert(":rabbit:", "🐰");
        m.insert(":fox:", "🦊");
        m.insert(":bear:", "🐻");
        m.insert(":panda:", "🐼");
        m.insert(":penguin:", "🐧");
        m.insert(":bird:", "🐦");
        m.insert(":duck:", "🦆");
        m.insert(":eagle:", "🦅");
        m.insert(":owl:", "🦉");
        m.insert(":bat:", "🦇");
        m.insert(":wolf:", "🐺");
        m.insert(":boar:", "🐗");
        m.insert(":horse:", "🐴");
        m.insert(":unicorn:", "🦄");
        m.insert(":bee:", "🐝");
        m.insert(":bug:", "🐛");
        m.insert(":butterfly:", "🦋");
        m.insert(":snail:", "🐌");
        m.insert(":worm:", "🪱");
        m.insert(":ladybug:", "🐞");
        m.insert(":ant:", "🐜");
        m.insert(":mosquito:", "🦟");
        m.insert(":cricket:", "🦗");
        m.insert(":spider:", "🕷");
        m.insert(":snake:", "🐍");
        m.insert(":lizard:", "🦎");
        m.insert(":turtle:", "🐢");
        m.insert(":shark:", "🦈");
        m.insert(":whale:", "🐳");
        m.insert(":dolphin:", "🐬");
        m.insert(":fish:", "🐟");
        m.insert(":octopus:", "🐙");
        m.insert(":crab:", "🦀");

        // Food & Drink
        m.insert(":apple:", "🍎");
        m.insert(":banana:", "🍌");
        m.insert(":grapes:", "🍇");
        m.insert(":watermelon:", "🍉");
        m.insert(":strawberry:", "🍓");
        m.insert(":peach:", "🍑");
        m.insert(":cherry:", "🍒");
        m.insert(":pineapple:", "🍍");
        m.insert(":coconut:", "🥥");
        m.insert(":avocado:", "🥑");
        m.insert(":pizza:", "🍕");
        m.insert(":hamburger:", "🍔");
        m.insert(":fries:", "🍟");
        m.insert(":taco:", "🌮");
        m.insert(":burrito:", "🌯");
        m.insert(":popcorn:", "🍿");
        m.insert(":ice_cream:", "🍦");
        m.insert(":donut:", "🍩");
        m.insert(":cookie:", "🍪");
        m.insert(":cake:", "🎂");
        m.insert(":chocolate:", "🍫");
        m.insert(":coffee:", "☕");
        m.insert(":tea:", "🍵");
        m.insert(":beer:", "🍺");
        m.insert(":wine:", "🍷");
        m.insert(":cocktail:", "🍸");
        m.insert(":tropical_drink:", "🍹");
        m.insert(":juice:", "🧃");

        // Activities & Objects
        m.insert(":soccer:", "⚽");
        m.insert(":basketball:", "🏀");
        m.insert(":football:", "🏈");
        m.insert(":baseball:", "⚾");
        m.insert(":tennis:", "🎾");
        m.insert(":golf:", "⛳");
        m.insert(":trophy:", "🏆");
        m.insert(":medal:", "🏅");
        m.insert(":first_place:", "🥇");
        m.insert(":second_place:", "🥈");
        m.insert(":third_place:", "🥉");
        m.insert(":dart:", "🎯");
        m.insert(":bowling:", "🎳");
        m.insert(":video_game:", "🎮");
        m.insert(":dice:", "🎲");
        m.insert(":chess:", "♟");
        m.insert(":art:", "🎨");
        m.insert(":guitar:", "🎸");
        m.insert(":microphone:", "🎤");
        m.insert(":headphones:", "🎧");
        m.insert(":trumpet:", "🎺");
        m.insert(":violin:", "🎻");
        m.insert(":drum:", "🥁");
        m.insert(":movie:", "🎬");
        m.insert(":ticket:", "🎫");

        // Nature & Weather
        m.insert(":seedling:", "🌱");
        m.insert(":herb:", "🌿");
        m.insert(":shamrock:", "☘");
        m.insert(":four_leaf_clover:", "🍀");
        m.insert(":bamboo:", "🎋");
        m.insert(":palm:", "🌴");
        m.insert(":cactus:", "🌵");
        m.insert(":tulip:", "🌷");
        m.insert(":rose:", "🌹");
        m.insert(":sunflower:", "🌻");
        m.insert(":hibiscus:", "🌺");
        m.insert(":blossom:", "🌼");
        m.insert(":cherry_blossom:", "🌸");
        m.insert(":mushroom:", "🍄");
        m.insert(":earth:", "🌍");
        m.insert(":moon:", "🌙");
        m.insert(":sun:", "☀");
        m.insert(":star:", "⭐");
        m.insert(":star2:", "🌟");
        m.insert(":sparkles:", "✨");
        m.insert(":zap:", "⚡");
        m.insert(":fire:", "🔥");
        m.insert(":flame:", "🔥");
        m.insert(":droplet:", "💧");
        m.insert(":water:", "🌊");
        m.insert(":rainbow:", "🌈");
        m.insert(":cloud:", "☁");
        m.insert(":rain:", "🌧");
        m.insert(":snow:", "❄");
        m.insert(":wind:", "💨");
        m.insert(":cyclone:", "🌀");
        m.insert(":fog:", "🌫");

        // Travel & Places
        m.insert(":car:", "🚗");
        m.insert(":bus:", "🚌");
        m.insert(":taxi:", "🚕");
        m.insert(":ambulance:", "🚑");
        m.insert(":truck:", "🚚");
        m.insert(":bike:", "🚲");
        m.insert(":airplane:", "✈");
        m.insert(":rocket:", "🚀");
        m.insert(":ufo:", "🛸");
        m.insert(":ship:", "🚢");
        m.insert(":house:", "🏠");
        m.insert(":office:", "🏢");
        m.insert(":hospital:", "🏥");
        m.insert(":school:", "🏫");
        m.insert(":church:", "⛪");
        m.insert(":mosque:", "🕌");
        m.insert(":temple:", "🛕");
        m.insert(":tent:", "⛺");
        m.insert(":ferris_wheel:", "🎡");
        m.insert(":roller_coaster:", "🎢");

        // Symbols
        m.insert(":check:", "✅");
        m.insert(":check_mark:", "✔");
        m.insert(":x:", "❌");
        m.insert(":cross_mark:", "✘");
        m.insert(":warning:", "⚠");
        m.insert(":no_entry:", "⛔");
        m.insert(":prohibited:", "🚫");
        m.insert(":100:", "💯");
        m.insert(":bang:", "❗");
        m.insert(":question:", "❓");
        m.insert(":grey_question:", "❔");
        m.insert(":grey_exclamation:", "❕");
        m.insert(":recycle:", "♻");
        m.insert(":trident:", "🔱");
        m.insert(":beginner:", "🔰");
        m.insert(":o:", "⭕");
        m.insert(":white_check:", "✅");
        m.insert(":loop:", "➰");
        m.insert(":1234:", "🔢");
        m.insert(":abc:", "🔤");
        m.insert(":abcd:", "🔡");
        m.insert(":capital_abcd:", "🔠");
        m.insert(":information:", "ℹ");
        m.insert(":hash:", "#");
        m.insert(":asterisk:", "*");
        m.insert(":zero:", "0");
        m.insert(":one:", "1");
        m.insert(":two:", "2");
        m.insert(":three:", "3");
        m.insert(":four:", "4");
        m.insert(":five:", "5");
        m.insert(":six:", "6");
        m.insert(":seven:", "7");
        m.insert(":eight:", "8");
        m.insert(":nine:", "9");
        m.insert(":keycap_ten:", "🔟");

        // Arrows & Controls
        m.insert(":arrow_up:", "⬆");
        m.insert(":arrow_down:", "⬇");
        m.insert(":arrow_left:", "⬅");
        m.insert(":arrow_right:", "➡");
        m.insert(":arrow_up_down:", "↕");
        m.insert(":arrow_left_right:", "↔");
        m.insert(":rewind:", "⏪");
        m.insert(":fast_forward:", "⏩");
        m.insert(":arrow_double_up:", "⏫");
        m.insert(":arrow_double_down:", "⏬");
        m.insert(":back:", "◀");
        m.insert(":end:", "🔚");
        m.insert(":on:", "🔛");
        m.insert(":soon:", "🔜");
        m.insert(":top:", "🔝");

        // Objects & Tools
        m.insert(":gem:", "💎");
        m.insert(":crown:", "👑");
        m.insert(":ring:", "💍");
        m.insert(":gift:", "🎁");
        m.insert(":balloon:", "🎈");
        m.insert(":tada:", "🎉");
        m.insert(":party:", "🎉");
        m.insert(":confetti:", "🎊");
        m.insert(":lantern:", "🏮");
        m.insert(":ribbon:", "🎀");
        m.insert(":lock:", "🔒");
        m.insert(":unlock:", "🔓");
        m.insert(":key:", "🔑");
        m.insert(":shield:", "🛡");
        m.insert(":wrench:", "🔧");
        m.insert(":hammer:", "🔨");
        m.insert(":nut_and_bolt:", "🔩");
        m.insert(":gear:", "⚙");
        m.insert(":link:", "🔗");
        m.insert(":paperclip:", "📎");
        m.insert(":scissors:", "✂");
        m.insert(":file:", "📄");
        m.insert(":folder:", "📁");
        m.insert(":notebook:", "📓");
        m.insert(":book:", "📖");
        m.insert(":pencil:", "✏");
        m.insert(":pen:", "🖊");
        m.insert(":crayon:", "🖍");
        m.insert(":paint:", "🎨");
        m.insert(":mag:", "🔍");
        m.insert(":bulb:", "💡");
        m.insert(":flashlight:", "🔦");
        m.insert(":battery:", "🔋");
        m.insert(":plug:", "🔌");
        m.insert(":computer:", "💻");
        m.insert(":desktop:", "🖥");
        m.insert(":keyboard:", "⌨");
        m.insert(":mouse_computer:", "🖱");
        m.insert(":printer:", "🖨");
        m.insert(":phone:", "📱");
        m.insert(":telephone:", "☎");
        m.insert(":camera:", "📷");
        m.insert(":video:", "📹");
        m.insert(":tv:", "📺");
        m.insert(":radio:", "📻");
        m.insert(":speaker:", "🔊");
        m.insert(":mute:", "🔇");
        m.insert(":bell:", "🔔");
        m.insert(":no_bell:", "🔕");
        m.insert(":mega:", "📣");
        m.insert(":hourglass:", "⏳");
        m.insert(":alarm:", "⏰");
        m.insert(":watch:", "⌚");
        m.insert(":calendar:", "📅");
        m.insert(":clock:", "🕐");

        // Flags & Misc
        m.insert(":white_flag:", "🏳");
        m.insert(":black_flag:", "🏴");
        m.insert(":checkered_flag:", "🏁");
        m.insert(":triangular_flag:", "🚩");
        m.insert(":rainbow_flag:", "🏳‍🌈");
        m.insert(":pirate_flag:", "🏴‍☠");

        m
    })
}

/// Expands `:shortcode:` patterns in `text` to their Unicode emoji equivalents.
/// Only processes shortcodes that exist in the map; unknown patterns are left as-is.
pub fn expand_shortcodes(text: &str) -> String {
    if !text.contains(':') {
        return text.to_string();
    }

    let map = emoji_map();
    let mut result = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut token = String::new();
    let mut in_token = false;

    while let Some(ch) = chars.next() {
        if ch == ':' {
            if in_token {
                // End of potential shortcode
                token.push(':');
                if let Some(&emoji) = map.get(token.as_str()) {
                    result.push_str(emoji);
                } else {
                    result.push_str(&token);
                }
                token.clear();
                in_token = false;
            } else {
                // Start of potential shortcode
                token.push(':');
                in_token = true;
            }
        } else if in_token {
            token.push(ch);
            if token.len() > 64 {
                // Too long for a valid shortcode, flush
                result.push_str(&token);
                token.clear();
                in_token = false;
            }
        } else {
            result.push(ch);
        }
    }

    // Flush remaining token
    if in_token {
        result.push_str(&token);
    }

    result
}

/// Returns all available shortcodes and their emoji, sorted alphabetically.
pub fn emoji_list() -> Vec<(&'static str, &'static str)> {
    let map = emoji_map();
    let mut entries: Vec<(&str, &str)> = map.iter().map(|(&k, &v)| (k, v)).collect();
    entries.sort_unstable_by_key(|(k, _)| *k);
    entries
}

/// Searches shortcodes containing `query` (case-insensitive).
pub fn search_emoji(query: &str) -> Vec<(&'static str, &'static str)> {
    let lower = query.to_lowercase();
    let map = emoji_map();
    let mut results: Vec<(&str, &str)> = map
        .iter()
        .filter(|(&k, _)| k.to_lowercase().contains(&lower))
        .map(|(&k, &v)| (k, v))
        .collect();
    results.sort_unstable_by_key(|(k, _)| *k);
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_known_shortcode() {
        assert_eq!(expand_shortcodes("Hello :wave:"), "Hello 👋");
    }

    #[test]
    fn test_expand_multiple_shortcodes() {
        assert_eq!(expand_shortcodes(":heart: :fire:"), "❤ 🔥");
    }

    #[test]
    fn test_unknown_shortcode_preserved() {
        assert_eq!(expand_shortcodes(":notarealemoji:"), ":notarealemoji:");
    }

    #[test]
    fn test_no_colons_unchanged() {
        assert_eq!(expand_shortcodes("plain text"), "plain text");
    }

    #[test]
    fn test_adjacent_emojis() {
        assert_eq!(expand_shortcodes(":star::star:"), "⭐⭐");
    }

    #[test]
    fn test_emoji_in_sentence() {
        assert_eq!(expand_shortcodes("I :heart: Rust :rocket:"), "I ❤ Rust 🚀");
    }

    #[test]
    fn test_expand_composed_flag_emojis() {
        assert_eq!(expand_shortcodes(":rainbow_flag:"), "🏳‍🌈");
        assert_eq!(expand_shortcodes(":pirate_flag:"), "🏴‍☠");
    }

    #[test]
    fn test_single_colon_unchanged() {
        assert_eq!(expand_shortcodes("50%: done"), "50%: done");
    }

    #[test]
    fn test_empty_shortcode_preserved() {
        assert_eq!(expand_shortcodes("::"), "::");
    }

    #[test]
    fn test_trailing_colon() {
        assert_eq!(expand_shortcodes("hello:"), "hello:");
    }

    #[test]
    fn test_emoji_list_sorted() {
        let list = emoji_list();
        for i in 1..list.len() {
            assert!(list[i].0 >= list[i - 1].0, "List not sorted at index {}", i);
        }
    }

    #[test]
    fn test_search_emoji_case_insensitive() {
        let results = search_emoji("WAVE");
        assert!(!results.is_empty());
        assert!(results.iter().any(|(k, _)| k.contains("wave")));
    }

    #[test]
    fn test_search_emoji_partial_match() {
        let results = search_emoji("heart");
        assert!(results.len() > 1);
    }

    #[test]
    fn test_search_emoji_no_match() {
        let results = search_emoji("zzzznotreal");
        assert!(results.is_empty());
    }
}
