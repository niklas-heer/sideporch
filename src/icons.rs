//! Interface icons from [Phosphor](https://phosphoricons.com) (MIT),
//! inlined as SVG so they inherit the text colour.

use maud::{Markup, PreEscaped};
pub use phosphor_svgs::style::regular::{
    ALARM, ARROW_BEND_UP_LEFT, ARROW_COUNTER_CLOCKWISE, ARROW_LEFT, ARROWS_CLOCKWISE, AT, BELL,
    BELL_SLASH, BOOKMARK_SIMPLE, BOOKS, CHAT_CIRCLE_TEXT, CLOCK, COMPASS, COPY, DOTS_THREE,
    DOWNLOAD_SIMPLE, FILE, GAUGE, GEAR_SIX, GIF, HASH, KEY, LIGHTNING, LINK_SIMPLE, LOCK_SIMPLE,
    MAGIC_WAND, MAGNIFYING_GLASS, PAPER_PLANE_RIGHT, PAPERCLIP, PENCIL_SIMPLE, PLAY, PLUS,
    PUSH_PIN, ROBOT, SIGN_OUT, SMILEY, SPARKLE, TRASH, USER_CIRCLE, USERS, WEBHOOKS_LOGO, X,
};

/// Renders `svg` with the given classes, hidden from assistive technology;
/// the surrounding control carries the accessible name.
pub fn icon(svg: &str, class: &str) -> Markup {
    PreEscaped(svg.replacen(
        "<svg ",
        &format!(r#"<svg class="{class}" aria-hidden="true" focusable="false" "#),
        1,
    ))
}
