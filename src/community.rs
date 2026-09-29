//! Who may do what: trust levels people earn by taking part, roles admins
//! hand out, the permissions both grant, and how people join.
//!
//! Admins may do everything. Anyone else may use a permission when their
//! trust level reaches the level the permission asks for, or when one of
//! their roles grants it. People start at level 0 when they sign up on
//! their own, or level 1 when someone invited them, and move up as they
//! stay and take part; admins can set a level and keep it there.

use rusqlite::{Connection, OptionalExtension as _, params};

use crate::{error::AppResult, store};

pub const DAY_MS: i64 = 24 * 60 * 60 * 1000;

/// Trust levels, from new to trusted.
pub const LEVELS: [(u8, &str, &str); 5] = [
    (0, "New", "Just signed up on their own."),
    (
        1,
        "Basic",
        "Invited, or has been around for a little while.",
    ),
    (2, "Member", "Takes part regularly."),
    (
        3,
        "Regular",
        "Has been part of the community for a long time.",
    ),
    (4, "Leader", "Given by an admin only."),
];

pub fn level_name(level: u8) -> &'static str {
    LEVELS
        .iter()
        .find(|(number, _, _)| *number == level)
        .map_or("Leader", |(_, name, _)| name)
}

/// Something only some people may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    UploadFiles,
    PostLinks,
    MentionEveryone,
    StartDirectMessages,
    CreateChannels,
    CreatePrivateChannels,
    CreatePolls,
    AddEmoji,
    InvitePeople,
    Moderate,
    ViewStatistics,
}

impl Permission {
    pub const ALL: [Self; 11] = [
        Self::UploadFiles,
        Self::PostLinks,
        Self::MentionEveryone,
        Self::StartDirectMessages,
        Self::CreateChannels,
        Self::CreatePrivateChannels,
        Self::CreatePolls,
        Self::AddEmoji,
        Self::InvitePeople,
        Self::Moderate,
        Self::ViewStatistics,
    ];

    pub const fn key(self) -> &'static str {
        match self {
            Self::UploadFiles => "upload_files",
            Self::PostLinks => "post_links",
            Self::MentionEveryone => "mention_everyone",
            Self::StartDirectMessages => "start_direct_messages",
            Self::CreateChannels => "create_channels",
            Self::CreatePrivateChannels => "create_private_channels",
            Self::CreatePolls => "create_polls",
            Self::AddEmoji => "add_emoji",
            Self::InvitePeople => "invite_people",
            Self::Moderate => "moderate",
            Self::ViewStatistics => "view_statistics",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|permission| permission.key() == key)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::UploadFiles => "Upload files and images",
            Self::PostLinks => "Post links",
            Self::MentionEveryone => "Notify everyone with @channel and @here",
            Self::StartDirectMessages => "Start direct messages",
            Self::CreateChannels => "Create public channels",
            Self::CreatePrivateChannels => "Create private channels",
            Self::CreatePolls => "Create polls",
            Self::AddEmoji => "Add custom emoji",
            Self::InvitePeople => "Invite people",
            Self::Moderate => "Moderate",
            Self::ViewStatistics => "See statistics",
        }
    }

    pub const fn description(self) -> &'static str {
        match self {
            Self::UploadFiles => "Turn this off entirely by allowing it for admins only.",
            Self::PostLinks => {
                "Messages with links are refused for others; spam usually needs them."
            }
            Self::MentionEveryone => "Mentions that notify a whole channel.",
            Self::StartDirectMessages => {
                "Anyone can still answer a conversation someone else started."
            }
            Self::CreateChannels
            | Self::CreatePrivateChannels
            | Self::CreatePolls
            | Self::AddEmoji => "",
            Self::InvitePeople => "Create invite links on the People page.",
            Self::Moderate => {
                "Handle reports, delete anyone's messages, time people out and approve sign-ups."
            }
            Self::ViewStatistics => {
                "How much is said in public channels, where, and who writes most."
            }
        }
    }

    /// The trust level that grants it unless an admin chose otherwise;
    /// `None` for admins and roles only.
    pub const fn default_level(self) -> Option<u8> {
        match self {
            Self::CreatePolls => Some(0),
            Self::UploadFiles
            | Self::PostLinks
            | Self::MentionEveryone
            | Self::StartDirectMessages
            | Self::CreateChannels
            | Self::CreatePrivateChannels
            | Self::AddEmoji
            | Self::ViewStatistics => Some(1),
            Self::InvitePeople | Self::Moderate => None,
        }
    }

    const fn bit(self) -> u16 {
        match self {
            Self::UploadFiles => 1,
            Self::PostLinks => 1 << 1,
            Self::MentionEveryone => 1 << 2,
            Self::StartDirectMessages => 1 << 3,
            Self::CreateChannels => 1 << 4,
            Self::CreatePrivateChannels => 1 << 5,
            Self::CreatePolls => 1 << 6,
            Self::AddEmoji => 1 << 7,
            Self::InvitePeople => 1 << 8,
            Self::Moderate => 1 << 9,
            Self::ViewStatistics => 1 << 10,
        }
    }
}

/// The permissions one person has.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Grants(u16);

impl Grants {
    pub const fn all() -> Self {
        Self(u16::MAX)
    }

    pub const fn has(self, permission: Permission) -> bool {
        self.0 & permission.bit() != 0
    }

    const fn with(self, permission: Permission) -> Self {
        Self(self.0 | permission.bit())
    }
}

/// Why someone may not do something yet.
pub fn refusal(permission: Permission) -> String {
    let what = match permission {
        Permission::UploadFiles => "upload files",
        Permission::PostLinks => "post links",
        Permission::MentionEveryone => "notify everyone with @channel or @here",
        Permission::StartDirectMessages => "start direct messages",
        Permission::CreateChannels => "create channels",
        Permission::CreatePrivateChannels => "create private channels",
        Permission::CreatePolls => "create polls",
        Permission::AddEmoji => "add custom emoji",
        Permission::InvitePeople => "invite people",
        Permission::Moderate => "moderate",
        Permission::ViewStatistics => "see statistics",
    };
    format!(
        "You can't {what} here yet. New members earn more as they take part; an admin can also allow it."
    )
}

/// The level each permission asks for, as configured.
pub fn levels(conn: &Connection) -> AppResult<Vec<(Permission, Option<u8>)>> {
    let mut statement = conn.prepare("SELECT permission, min_level FROM permission_levels")?;
    let chosen: Vec<(String, Option<u8>)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect::<Result<_, _>>()?;
    Ok(Permission::ALL
        .into_iter()
        .map(|permission| {
            let level = chosen
                .iter()
                .find(|(key, _)| key == permission.key())
                .map_or_else(|| permission.default_level(), |(_, level)| *level);
            (permission, level)
        })
        .collect())
}

pub fn set_level(conn: &Connection, permission: Permission, level: Option<u8>) -> AppResult<()> {
    conn.execute(
        "INSERT INTO permission_levels (permission, min_level) VALUES (?1, ?2)
         ON CONFLICT (permission) DO UPDATE SET min_level = excluded.min_level",
        params![permission.key(), level],
    )?;
    Ok(())
}

/// What someone may do, from their level and roles.
pub fn grants(conn: &Connection, user_id: i64, is_admin: bool, level: u8) -> AppResult<Grants> {
    if is_admin {
        return Ok(Grants::all());
    }
    let mut grants = Grants::default();
    for (permission, needed) in levels(conn)? {
        if needed.is_some_and(|needed| level >= needed) {
            grants = grants.with(permission);
        }
    }
    let mut statement = conn.prepare(
        "SELECT DISTINCT p.permission FROM role_permissions p
         JOIN user_roles r ON r.role_id = p.role_id WHERE r.user_id = ?1",
    )?;
    let granted = statement.query_map([user_id], |row| row.get::<_, String>(0))?;
    for key in granted {
        if let Some(permission) = Permission::from_key(&key?) {
            grants = grants.with(permission);
        }
    }
    Ok(grants)
}

// Trust levels

/// What it takes to reach a level on your own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement {
    /// Days since the account was created.
    pub days: u32,
    /// Days on which they opened Sideporch.
    pub visits: u32,
    pub messages: u32,
}

/// Defaults for levels 1, 2 and 3.
const REQUIREMENTS: [Requirement; 3] = [
    Requirement {
        days: 1,
        visits: 1,
        messages: 3,
    },
    Requirement {
        days: 7,
        visits: 3,
        messages: 20,
    },
    Requirement {
        days: 30,
        visits: 15,
        messages: 100,
    },
];

fn number(conn: &Connection, key: &str, default: u32) -> AppResult<u32> {
    Ok(store::setting(conn, key)?
        .and_then(|value| value.parse().ok())
        .unwrap_or(default))
}

/// A level `user_id` reached and wasn't told about yet, with the
/// permissions it brought. The first time, it just notes the current level.
pub fn level_up(conn: &Connection, user_id: i64) -> AppResult<Option<(u8, Vec<&'static str>)>> {
    let (level, noticed): (u8, Option<u8>) = conn.query_row(
        "SELECT trust_level, level_noticed FROM users WHERE id = ?1",
        [user_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    let Some(noticed) = noticed else {
        notice_level(conn, user_id)?;
        return Ok(None);
    };
    if level <= noticed {
        return Ok(None);
    }
    let unlocked = levels(conn)?
        .into_iter()
        .filter(|(_, needed)| needed.is_some_and(|needed| needed > noticed && needed <= level))
        .map(|(permission, _)| permission.label())
        .collect();
    Ok(Some((level, unlocked)))
}

/// Remembers that `user_id` saw their current level.
pub fn notice_level(conn: &Connection, user_id: i64) -> AppResult<()> {
    conn.execute(
        "UPDATE users SET level_noticed = trust_level WHERE id = ?1",
        [user_id],
    )?;
    Ok(())
}

/// What `progress` still needs for the next level, or `None` at the top,
/// or when an admin keeps them where they are.
pub fn next_level(conn: &Connection, progress: &Progress) -> AppResult<Option<(u8, Requirement)>> {
    if progress.locked {
        return Ok(None);
    }
    let requirements = requirements(conn)?;
    let Some(next) = requirements.get(usize::from(progress.level)) else {
        return Ok(None);
    };
    Ok(Some((
        progress.level.saturating_add(1),
        Requirement {
            days: next.days.saturating_sub(progress.days),
            visits: next.visits.saturating_sub(progress.visits),
            messages: next.messages.saturating_sub(progress.messages),
        },
    )))
}

/// The requirements for levels 1 to 3.
pub fn requirements(conn: &Connection) -> AppResult<[Requirement; 3]> {
    let mut all = REQUIREMENTS;
    for (level, requirement) in (1_u8..).zip(all.iter_mut()) {
        requirement.days = number(conn, &format!("trust.{level}.days"), requirement.days)?;
        requirement.visits = number(conn, &format!("trust.{level}.visits"), requirement.visits)?;
        requirement.messages = number(
            conn,
            &format!("trust.{level}.messages"),
            requirement.messages,
        )?;
    }
    Ok(all)
}

pub fn set_requirements(conn: &Connection, all: &[Requirement; 3]) -> AppResult<()> {
    for (level, requirement) in (1_u8..).zip(all) {
        store::set_setting(
            conn,
            &format!("trust.{level}.days"),
            &requirement.days.to_string(),
        )?;
        store::set_setting(
            conn,
            &format!("trust.{level}.visits"),
            &requirement.visits.to_string(),
        )?;
        store::set_setting(
            conn,
            &format!("trust.{level}.messages"),
            &requirement.messages.to_string(),
        )?;
    }
    Ok(())
}

/// How far someone has come, for promotions and their profile.
#[derive(Debug, Clone, Copy, Default)]
pub struct Progress {
    pub level: u8,
    pub locked: bool,
    pub days: u32,
    pub visits: u32,
    pub messages: u32,
}

pub fn progress(conn: &Connection, user_id: i64, now: i64) -> AppResult<Option<Progress>> {
    Ok(conn
        .query_row(
            "SELECT trust_level, trust_locked, created_at, days_visited,
                    (SELECT COUNT(*) FROM messages m WHERE m.user_id = u.id AND m.deleted_at IS NULL)
             FROM users u WHERE id = ?1",
            [user_id],
            |row| {
                let created: i64 = row.get(2)?;
                let days = now.saturating_sub(created).checked_div(DAY_MS).unwrap_or(0);
                Ok(Progress {
                    level: row.get(0)?,
                    locked: row.get(1)?,
                    days: u32::try_from(days).unwrap_or(u32::MAX),
                    visits: row.get(3)?,
                    messages: u32::try_from(row.get::<_, i64>(4)?).unwrap_or(u32::MAX),
                })
            },
        )
        .optional()?)
}

/// The level someone's progress earns. Level 4 is never earned.
pub fn earned(progress: &Progress, requirements: &[Requirement; 3]) -> u8 {
    let mut level = 0;
    for (next, requirement) in (1_u8..).zip(requirements) {
        if progress.days >= requirement.days
            && progress.visits >= requirement.visits
            && progress.messages >= requirement.messages
        {
            level = next;
        } else {
            break;
        }
    }
    level
}

/// Promotes someone who earned a higher level. Levels never go down on
/// their own, and a level an admin locked stays. Returns the new level.
pub fn refresh(conn: &Connection, user_id: i64, now: i64) -> AppResult<Option<u8>> {
    // Levels above 3 aren't earned, so most active people skip counting.
    let settled: bool = conn
        .query_row(
            "SELECT trust_level >= 3 OR trust_locked FROM users WHERE id = ?1",
            [user_id],
            |row| row.get(0),
        )
        .optional()?
        .unwrap_or(true);
    if settled {
        return Ok(None);
    }
    let Some(progress) = progress(conn, user_id, now)? else {
        return Ok(None);
    };
    if progress.locked {
        return Ok(None);
    }
    let earned = earned(&progress, &requirements(conn)?);
    if earned <= progress.level {
        return Ok(None);
    }
    conn.execute(
        "UPDATE users SET trust_level = ?1 WHERE id = ?2",
        params![earned, user_id],
    )?;
    Ok(Some(earned))
}

/// Counts today as a visit, once a day, and checks for a promotion.
pub fn record_visit(conn: &Connection, user_id: i64, now: i64) -> AppResult<()> {
    let today = now.checked_div(DAY_MS).unwrap_or(0).to_string();
    let counted = conn.execute(
        "UPDATE users SET days_visited = days_visited + 1, last_visit_day = ?1
         WHERE id = ?2 AND last_visit_day != ?1",
        params![today, user_id],
    )?;
    if counted > 0 {
        refresh(conn, user_id, now)?;
    }
    Ok(())
}

/// Sets someone's level and whether it stays there.
pub fn set_trust(conn: &Connection, user_id: i64, level: u8, locked: bool) -> AppResult<()> {
    conn.execute(
        "UPDATE users SET trust_level = ?1, trust_locked = ?2 WHERE id = ?3",
        params![level.min(4), locked, user_id],
    )?;
    Ok(())
}

// Roles

#[derive(Debug, Clone)]
pub struct Role {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub permissions: Vec<Permission>,
    pub members: i64,
    /// Whether it shows as a badge next to its people's names.
    pub badge: bool,
    /// A key from [`BADGE_COLORS`].
    pub color: String,
}

/// Colors a role's badge can have: key, name, and the classes that draw it.
pub const BADGE_COLORS: [(&str, &str, &str); 6] = [
    (
        "green",
        "Green",
        "bg-emerald-100 text-emerald-900 dark:bg-emerald-950 dark:text-emerald-200",
    ),
    (
        "blue",
        "Blue",
        "bg-sky-100 text-sky-900 dark:bg-sky-950 dark:text-sky-200",
    ),
    (
        "purple",
        "Purple",
        "bg-violet-100 text-violet-900 dark:bg-violet-950 dark:text-violet-200",
    ),
    (
        "amber",
        "Amber",
        "bg-amber-100 text-amber-900 dark:bg-amber-950 dark:text-amber-200",
    ),
    (
        "rose",
        "Rose",
        "bg-rose-100 text-rose-900 dark:bg-rose-950 dark:text-rose-200",
    ),
    (
        "gray",
        "Gray",
        "bg-screen text-ink dark:bg-night-2 dark:text-haint-2",
    ),
];

/// The classes for a badge of `color`; admins' badge for an empty one.
pub fn badge_classes(color: &str) -> &'static str {
    BADGE_COLORS
        .iter()
        .find(|(key, _, _)| *key == color)
        .map_or(
            "bg-haint-2 text-floor dark:bg-floor-2 dark:text-haint-2",
            |(_, _, classes)| classes,
        )
}

pub fn set_role_badge(conn: &Connection, role_id: i64, badge: bool, color: &str) -> AppResult<()> {
    let color = if BADGE_COLORS.iter().any(|(key, _, _)| *key == color) {
        color
    } else {
        "gray"
    };
    conn.execute(
        "UPDATE roles SET badge = ?1, color = ?2 WHERE id = ?3",
        params![badge, color, role_id],
    )?;
    Ok(())
}

pub fn roles(conn: &Connection) -> AppResult<Vec<Role>> {
    let mut statement = conn.prepare(
        "SELECT r.id, r.name, r.description, (SELECT COUNT(*) FROM user_roles ur WHERE ur.role_id = r.id),
                r.badge, r.color
         FROM roles r ORDER BY r.name COLLATE NOCASE",
    )?;
    let mut roles: Vec<Role> = statement
        .query_map([], |row| {
            Ok(Role {
                id: row.get(0)?,
                name: row.get(1)?,
                description: row.get(2)?,
                permissions: Vec::new(),
                members: row.get(3)?,
                badge: row.get(4)?,
                color: row.get(5)?,
            })
        })?
        .collect::<Result<_, _>>()?;
    let mut statement = conn.prepare("SELECT role_id, permission FROM role_permissions")?;
    let granted = statement.query_map([], |row| {
        Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
    })?;
    for grant in granted {
        let (role_id, key) = grant?;
        if let (Some(role), Some(permission)) = (
            roles.iter_mut().find(|role| role.id == role_id),
            Permission::from_key(&key),
        ) {
            role.permissions.push(permission);
        }
    }
    Ok(roles)
}

pub fn create_role(conn: &Connection, name: &str, description: &str, now: i64) -> AppResult<i64> {
    conn.execute(
        "INSERT INTO roles (name, description, created_at) VALUES (?1, ?2, ?3)",
        params![name, description, now],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn delete_role(conn: &Connection, role_id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM roles WHERE id = ?1", [role_id])?;
    Ok(())
}

pub fn role_name_taken(conn: &Connection, name: &str) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM roles WHERE name = ?1)",
        [name],
        |row| row.get(0),
    )?)
}

pub fn set_role_permissions(
    conn: &Connection,
    role_id: i64,
    permissions: &[Permission],
) -> AppResult<()> {
    conn.execute("DELETE FROM role_permissions WHERE role_id = ?1", [role_id])?;
    for permission in permissions {
        conn.execute(
            "INSERT INTO role_permissions (role_id, permission) VALUES (?1, ?2)",
            params![role_id, permission.key()],
        )?;
    }
    Ok(())
}

/// The ids of someone's roles.
pub fn user_roles(conn: &Connection, user_id: i64) -> AppResult<Vec<i64>> {
    let mut statement = conn.prepare("SELECT role_id FROM user_roles WHERE user_id = ?1")?;
    let ids = statement.query_map([user_id], |row| row.get(0))?;
    Ok(ids.collect::<Result<_, _>>()?)
}

pub fn set_user_roles(conn: &Connection, user_id: i64, role_ids: &[i64]) -> AppResult<()> {
    conn.execute("DELETE FROM user_roles WHERE user_id = ?1", [user_id])?;
    for role_id in role_ids {
        conn.execute(
            "INSERT OR IGNORE INTO user_roles (user_id, role_id)
             SELECT ?1, id FROM roles WHERE id = ?2",
            params![user_id, role_id],
        )?;
    }
    Ok(())
}

// Joining

/// How people get an account.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Registration {
    /// Only with an invite link.
    #[default]
    Invite,
    /// Anyone can sign up.
    Open,
    /// Anyone can ask; an admin or moderator approves.
    Approval,
}

impl Registration {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "invite" => Some(Self::Invite),
            "open" => Some(Self::Open),
            "approval" => Some(Self::Approval),
            _ => None,
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Invite => "invite",
            Self::Open => "open",
            Self::Approval => "approval",
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Joining {
    pub registration: Registration,
    /// Markdown people agree to when signing up; empty for none.
    pub rules: String,
    /// Messages a level-0 member may send per minute; 0 for no limit.
    pub new_member_per_minute: u32,
}

impl Joining {
    pub fn load(conn: &Connection) -> AppResult<Self> {
        Ok(Self {
            registration: store::setting(conn, "registration.mode")?
                .as_deref()
                .and_then(Registration::parse)
                .unwrap_or_default(),
            rules: store::setting(conn, "registration.rules")?.unwrap_or_default(),
            new_member_per_minute: number(conn, "limits.new_member_per_minute", 6)?,
        })
    }

    pub fn save(&self, conn: &Connection) -> AppResult<()> {
        store::set_setting(conn, "registration.mode", self.registration.key())?;
        store::set_setting(conn, "registration.rules", self.rules.trim())?;
        store::set_setting(
            conn,
            "limits.new_member_per_minute",
            &self.new_member_per_minute.to_string(),
        )
    }
}

/// Sign-ups allowed per hour across the whole server, so a script can't
/// create accounts by the thousand.
pub const SIGNUPS_PER_HOUR: i64 = 30;

/// Whether the hourly limit on sign-ups is reached.
pub fn too_many_signups(conn: &Connection, now: i64) -> AppResult<bool> {
    let since = now.saturating_sub(60 * 60 * 1000);
    let recent: i64 = conn.query_row(
        "SELECT (SELECT COUNT(*) FROM users WHERE created_at > ?1 AND trust_level = 0)
              + (SELECT COUNT(*) FROM signups WHERE created_at > ?1)",
        [since],
        |row| row.get(0),
    )?;
    Ok(recent >= SIGNUPS_PER_HOUR)
}

/// Someone who asked to join and waits for approval.
#[derive(Debug, Clone)]
pub struct Signup {
    pub id: i64,
    pub username: String,
    pub display_name: String,
    pub note: String,
    pub created_at: i64,
}

pub fn signups(conn: &Connection) -> AppResult<Vec<Signup>> {
    let mut statement = conn
        .prepare("SELECT id, username, display_name, note, created_at FROM signups ORDER BY id")?;
    let rows = statement.query_map([], |row| {
        Ok(Signup {
            id: row.get(0)?,
            username: row.get(1)?,
            display_name: row.get(2)?,
            note: row.get(3)?,
            created_at: row.get(4)?,
        })
    })?;
    Ok(rows.collect::<Result<_, _>>()?)
}

pub fn add_signup(
    conn: &Connection,
    username: &str,
    display_name: &str,
    password_hash: &str,
    note: &str,
    now: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO signups (username, display_name, password_hash, note, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![username, display_name, password_hash, note, now],
    )?;
    Ok(())
}

/// Whether a username is taken by an account or a waiting sign-up.
pub fn username_taken(conn: &Connection, username: &str) -> AppResult<bool> {
    Ok(store::username_taken(conn, username)?
        || conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM signups WHERE username = ?1)",
            [username],
            |row| row.get(0),
        )?)
}

/// Turns a waiting sign-up into an account at level 1: someone vouched
/// for them. Returns the new account's id and name.
pub fn approve(
    conn: &Connection,
    signup_id: i64,
    now: i64,
) -> AppResult<Option<(i64, String, String)>> {
    let found: Option<(String, String, String)> = conn
        .query_row(
            "SELECT username, display_name, password_hash FROM signups WHERE id = ?1",
            [signup_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((username, display_name, hash)) = found else {
        return Ok(None);
    };
    conn.execute("DELETE FROM signups WHERE id = ?1", [signup_id])?;
    if store::username_taken(conn, &username)? {
        return Ok(None);
    }
    let id = store::create_user(conn, &username, &display_name, &hash, false, now)?;
    Ok(Some((id, display_name, username)))
}

pub fn decline(conn: &Connection, signup_id: i64) -> AppResult<()> {
    conn.execute("DELETE FROM signups WHERE id = ?1", [signup_id])?;
    Ok(())
}

/// Whether someone with this username waits for approval, to tell them at
/// the login form.
pub fn is_waiting(conn: &Connection, username: &str) -> AppResult<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS (SELECT 1 FROM signups WHERE username = ?1)",
        [username],
        |row| row.get(0),
    )?)
}

// Limits and moderation

/// Whether text holds a web link.
pub fn has_link(text: &str) -> bool {
    let lower = text.to_lowercase();
    ["http://", "https://", "www."]
        .iter()
        .any(|start| lower.contains(start))
}

/// Refuses someone who is timed out.
pub fn check_not_timed_out(user: &crate::auth::CurrentUser) -> AppResult<()> {
    user.timed_out_until.map_or(Ok(()), |until| {
        Err(crate::error::AppError::bad_request(format!(
            "Your posting is paused until {}. Moderators time people out, and so do reports about newcomers from several people.",
            jiff::Timestamp::from_millisecond(until)
                .unwrap_or_default()
                .strftime("%Y-%m-%d %H:%M UTC")
        )))
    })
}

/// Checks text someone is about to post or change a message to.
pub fn check_content(user: &crate::auth::CurrentUser, body: &str) -> AppResult<()> {
    check_not_timed_out(user)?;
    if has_link(body) {
        user.require(Permission::PostLinks)?;
    }
    if store::mentions_everyone(&body.to_lowercase()) {
        user.require(Permission::MentionEveryone)?;
    }
    Ok(())
}

/// Checks what someone is about to post: whether they may post at all
/// right now, and may use what the message holds.
pub fn check_message(
    conn: &Connection,
    user: &crate::auth::CurrentUser,
    body: &str,
    files: bool,
    now: i64,
) -> AppResult<()> {
    check_content(user, body)?;
    if files {
        user.require(Permission::UploadFiles)?;
    }
    if user.trust_level == 0 && !user.is_admin && over_new_member_limit(conn, user.id, now)? {
        return Err(crate::error::AppError::bad_request(
            "New members can send a few messages a minute. Wait a moment and try again.",
        ));
    }
    if !user.is_admin && repeats(conn, user.id, body, now)? >= REPEATS_ALLOWED {
        return Err(crate::error::AppError::bad_request(
            "You sent this same message twice in the last ten minutes. Say something new, or wait a little.",
        ));
    }
    Ok(())
}

/// How often one message may be sent again within [`REPEAT_WINDOW_MS`];
/// the third time is refused, which stops copy-and-paste spam.
const REPEATS_ALLOWED: i64 = 2;
const REPEAT_WINDOW_MS: i64 = 10 * 60 * 1000;
/// Shorter messages, like "ok" or "+1", may repeat freely.
const REPEAT_MIN_CHARS: usize = 12;

/// How many times `user_id` sent `body` recently.
fn repeats(conn: &Connection, user_id: i64, body: &str, now: i64) -> AppResult<i64> {
    let body = body.trim();
    if body.chars().count() < REPEAT_MIN_CHARS {
        return Ok(0);
    }
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE user_id = ?1 AND created_at > ?2
           AND deleted_at IS NULL AND trim(body) = ?3",
        params![user_id, now.saturating_sub(REPEAT_WINDOW_MS), body],
        |row| row.get(0),
    )?)
}

/// Distinct people whose reports time out a newcomer until a moderator
/// looks, and for how long.
const REPORTS_FOR_TIMEOUT: i64 = 2;
const REPORT_TIMEOUT_MS: i64 = 24 * 60 * 60 * 1000;

/// Whether a level-0 member already sent as many messages this minute as
/// they may.
pub fn over_new_member_limit(conn: &Connection, user_id: i64, now: i64) -> AppResult<bool> {
    let limit = number(conn, "limits.new_member_per_minute", 6)?;
    if limit == 0 {
        return Ok(false);
    }
    let sent: i64 = conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE user_id = ?1 AND created_at > ?2",
        params![user_id, now.saturating_sub(60_000)],
        |row| row.get(0),
    )?;
    Ok(sent >= i64::from(limit))
}

pub fn set_timeout(conn: &Connection, user_id: i64, until: Option<i64>) -> AppResult<()> {
    conn.execute(
        "UPDATE users SET muted_until = ?1 WHERE id = ?2",
        params![until, user_id],
    )?;
    Ok(())
}

/// How long moderators can time someone out for.
pub const TIMEOUTS: &[(i64, &str)] = &[
    (60 * 60 * 1000, "1 hour"),
    (24 * 60 * 60 * 1000, "1 day"),
    (7 * 24 * 60 * 60 * 1000, "1 week"),
];

#[derive(Debug, Clone)]
pub struct Report {
    pub message: store::Message,
    pub reporter: String,
    pub reason: String,
    pub created_at: i64,
    pub channel: String,
}

pub fn report(
    conn: &Connection,
    message_id: i64,
    reporter: i64,
    reason: &str,
    now: i64,
) -> AppResult<()> {
    conn.execute(
        "INSERT INTO reports (message_id, reporter_id, reason, created_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT (message_id, reporter_id) DO UPDATE SET reason = excluded.reason, resolved_at = NULL",
        params![message_id, reporter, reason, now],
    )?;
    // Newcomers whom several people report pause until a moderator looks.
    let newcomer: Option<(i64, i64)> = conn
        .query_row(
            "SELECT u.id, (SELECT COUNT(DISTINCT r.reporter_id) FROM reports r
                           JOIN messages rm ON rm.id = r.message_id
                           WHERE rm.user_id = u.id AND r.resolved_at IS NULL)
             FROM messages m JOIN users u ON u.id = m.user_id
             WHERE m.id = ?1 AND u.trust_level = 0 AND u.is_admin = 0
               AND (u.muted_until IS NULL OR u.muted_until < ?2)",
            params![message_id, now],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    if let Some((author, reporters)) = newcomer
        && reporters >= REPORTS_FOR_TIMEOUT
    {
        set_timeout(conn, author, Some(now.saturating_add(REPORT_TIMEOUT_MS)))?;
    }
    Ok(())
}

/// Reports nobody has handled yet, oldest first.
pub fn open_reports(conn: &Connection) -> AppResult<Vec<Report>> {
    let mut statement = conn.prepare(
        "SELECT r.message_id, COALESCE(u.display_name, 'Someone'), r.reason, r.created_at,
                COALESCE(c.name, 'a conversation')
         FROM reports r JOIN messages m ON m.id = r.message_id JOIN channels c ON c.id = m.channel_id
         LEFT JOIN users u ON u.id = r.reporter_id
         WHERE r.resolved_at IS NULL ORDER BY r.id",
    )?;
    let rows: Vec<(i64, String, String, i64, String)> = statement
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<Result<_, _>>()?;
    let mut reports = Vec::with_capacity(rows.len());
    for (message_id, reporter, reason, created_at, channel) in rows {
        if let Some(message) = store::message(conn, message_id)? {
            reports.push(Report {
                message,
                reporter,
                reason,
                created_at,
                channel,
            });
        }
    }
    Ok(reports)
}

/// Closes every open report about a message.
pub fn resolve_reports(
    conn: &Connection,
    message_id: i64,
    by: i64,
    outcome: &str,
    now: i64,
) -> AppResult<()> {
    conn.execute(
        "UPDATE reports SET resolved_at = ?1, resolved_by = ?2, outcome = ?3
         WHERE message_id = ?4 AND resolved_at IS NULL",
        params![now, by, outcome, message_id],
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_are_earned_in_order() {
        let progress = |days, visits, messages| Progress {
            days,
            visits,
            messages,
            ..Progress::default()
        };
        assert_eq!(earned(&progress(0, 1, 50), &REQUIREMENTS), 0);
        assert_eq!(earned(&progress(1, 1, 3), &REQUIREMENTS), 1);
        assert_eq!(earned(&progress(8, 3, 19), &REQUIREMENTS), 1);
        assert_eq!(earned(&progress(8, 3, 20), &REQUIREMENTS), 2);
        // Level 3 needs level 2's requirements too.
        let odd = [
            REQUIREMENTS[0],
            Requirement {
                days: 100,
                visits: 0,
                messages: 0,
            },
            Requirement {
                days: 0,
                visits: 0,
                messages: 0,
            },
        ];
        assert_eq!(earned(&progress(5, 5, 5), &odd), 1);
    }

    #[test]
    fn grants_are_bits() {
        let grants = Grants::default().with(Permission::PostLinks);
        assert!(grants.has(Permission::PostLinks));
        assert!(!grants.has(Permission::UploadFiles));
        assert!(
            Permission::ALL
                .iter()
                .all(|permission| Grants::all().has(*permission))
        );
        for permission in Permission::ALL {
            assert_eq!(Permission::from_key(permission.key()), Some(permission));
        }
    }
}
