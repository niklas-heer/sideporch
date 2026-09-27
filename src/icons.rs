//! Interface icons from [Phosphor](https://phosphoricons.com) (MIT),
//! inlined as SVG so they inherit the text colour.

use maud::{Markup, PreEscaped};
pub use phosphor_svgs::style::regular::{
    ARROW_BEND_UP_LEFT, ARROW_LEFT, BELL, CHAT_CIRCLE_TEXT, COPY, DOWNLOAD_SIMPLE, FILE, GEAR_SIX,
    HASH, LIGHTNING, LINK_SIMPLE, MAGNIFYING_GLASS, PAPER_PLANE_RIGHT, PAPERCLIP, PLUS, ROBOT,
    SIGN_OUT, SMILEY, TRASH, USERS, WEBHOOKS_LOGO, X,
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
