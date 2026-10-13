# Design System — Codeline Intranet

## 1. Visual Theme & Atmosphere

The Codeline Intranet is a dark, vampiric workspace built on the vampiric palette — a deep violet-black night (`#22212C`, `#1B1A23`, `#454158`) where content glows in phosphorescent neons. Where Spotify hides in achromatic charcoal, Default visual theme embraces a **tinted darkness**: every background carries a whisper of purple, making the interface feel warm and alive rather than void. The design philosophy is "neon on velvet" — muted violet surfaces recede while purple (`#9580FF`), pink (`#FF80BF`), green (`#8AFF80`) and cyan (`#80FFEA`) accents pulse like signage in the dark.

Typography is developer-native: a clean geometric sans (**Inter**) for UI and reading, with **JetBrains Mono** reserved for code, timestamps, statuses and bot messages — a nod to the theme's editor heritage. Weight does the hierarchy work: 700 for titles and active states, 600 for section heads and buttons, 400 for body. The off-white foreground (`#F8F8F2`) is never pure white; it sits softly on the violet dark, and secondary text drops to the muted purple-gray comment color (`#7970A9`).

What distinguishes this system is its **functional neon assignment**. Each accent color has a job: purple is the brand and primary action, green means alive/success/online, red-salmon (`#FF9580`) means down/error, pink is real-time activity (mentions, live voice, scrollbars), cyan is links and bots, orange (`#FFCA80`) and yellow (`#FFFF80`) are warnings. Surfaces are soft-rounded (8px–12px), elevation comes from lighter violet steps rather than heavy shadows, and glows — not gray borders — mark focus.

**Key Characteristics:**
- Violet-black immersive dark theme (`#1B1A23`–`#454158`) — darkness with a purple soul
- Purple (`#9580FF`) as the primary brand accent — actions, active nav, focus
- Full neon semantic set: green = up/success, red `#FF9580` = down/error, orange/yellow = warning, cyan = info/links/bots, pink = live activity
- Off-white foreground (`#F8F8F2`) — never pure `#FFFFFF`
- Muted purple (`#7970A9`) for secondary text — "comments" in the code sense
- Inter for UI, JetBrains Mono for code/status/bot voice
- Soft radii (8px–12px), full circles (50%) for avatars and voice controls
- Elevation via lighter surface steps + subtle black shadows; focus via neon glow rings

## 2. Color Palette & Roles

### Primary Brand
- **Purple** (`#9580FF`): Primary brand accent — primary buttons, active nav items, focus rings, selected states
- **Night** (`#22212C`): Base background surface (app background)
- **Deep Night** (`#1B1A23`): Deepest layer — sidebars, wells, recessed areas
- **Selection** (`#454158`): Active/selected item backgrounds, hover-strong, input borders

### Surfaces
- **Card** (`#2A2937`): Cards, post containers, channel list, elevated surfaces
- **Card Raised** (`#313041`): Hover state on cards, dropdowns, popovers
- **Selection** (`#454158`): Selected rows, active channel, pressed states

### Text
- **Foreground** (`#F8F8F2`): Primary text — soft off-white
- **Comment** (`#7970A9`): Secondary text, muted labels, timestamps, placeholders, inactive nav
- **Purple Tint** (`#9580FF` at 50% — `#9580FF7F`): Tertiary/disabled text on dark

### Semantic (the neons)
- **Green** (`#8AFF80`): Success, app **UP**, online presence, connected voice
- **Red** (`#FF9580`): Error, app **DOWN**, destructive actions, missed/failed
- **Orange** (`#FFCA80`): Warning, degraded status, pending checks
- **Yellow** (`#FFFF80`): Attention highlights, unread markers, starred
- **Cyan** (`#80FFEA`): Links, informational badges, **bot identity**, meeting links
- **Pink** (`#FF80BF`): Live/real-time — mentions, active voice pulse, recording, scrollbars, notification badges

### Borders & Lines
- **Border** (`#454158`): Standard border on inputs and dividers between zones
- **Border Subtle** (`#2A2937`): Hairline separators inside cards, list dividers
- **Black Veil** (`rgba(0,0,0,0.5)`): Outer window/dialog borders

### Shadows & Glows
- **Card Shadow** (`rgba(0,0,0,0.3) 0px 4px 12px`): Cards, dropdowns
- **Dialog Shadow** (`rgba(0,0,0,0.5) 0px 8px 24px`): Modals, overlays
- **Purple Glow** (`rgba(149,128,255,0.4) 0px 0px 0px 3px`): Focus rings
- **Pink Glow** (`rgba(255,128,191,0.5) 0px 0px 12px`): Live voice indicator pulse
- **Green Glow** (`rgba(138,255,128,0.4) 0px 0px 8px`): App "UP" status dot
- **Red Glow** (`rgba(255,149,128,0.5) 0px 0px 8px`): App "DOWN" status dot

## 3. Typography Rules

### Font Families
- **UI / Body**: `Inter, -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif`
- **Mono (code, status, timestamps, bot messages)**: `"JetBrains Mono", "Fira Code", ui-monospace, SFMono-Regular, Menlo, monospace`

### Hierarchy

| Role | Font | Size | Weight | Line Height | Color | Notes |
|------|------|------|--------|-------------|-------|-------|
| Page Title | Inter | 24px (1.50rem) | 700 | 1.20 | `#F8F8F2` | Section headers (Home, Calendar…) |
| Section Heading | Inter | 18px (1.13rem) | 600 | 1.30 | `#F8F8F2` | Card titles, modal titles |
| Post Author / Emphasis | Inter | 16px (1.00rem) | 700 | 1.40 | `#F8F8F2` | Names, emphasized text |
| Body | Inter | 16px (1.00rem) | 400 | 1.50 | `#F8F8F2` | Posts, messages, descriptions |
| Nav Link Active | Inter | 14px (0.88rem) | 700 | 1.40 | `#F8F8F2` | Sidebar active item |
| Nav Link | Inter | 14px (0.88rem) | 400 | 1.40 | `#7970A9` | Sidebar inactive item |
| Button | Inter | 14px (0.88rem) | 600 | 1.00 | varies | Letter-spacing 0.3px |
| Channel Name | Inter | 14px (0.88rem) | 600 | 1.40 | `#7970A9` / `#F8F8F2` active | `#` prefix in `#7970A9` |
| Caption / Meta | Inter | 13px (0.81rem) | 400 | 1.40 | `#7970A9` | Post meta, attendee counts |
| Timestamp / Status | JetBrains Mono | 12px (0.75rem) | 400 | 1.50 | `#7970A9` | Message times, check intervals |
| Badge / Tag | Inter | 11px (0.69rem) | 600 | 1.30 | varies | Uppercase, letter-spacing 0.5px |
| Bot Label | JetBrains Mono | 10px (0.63rem) | 700 | 1.00 | `#22212C` on `#80FFEA` | "BOT" chip, uppercase |

### Principles
- **Weight over size**: hierarchy is built with 700/600/400 contrast; the size range stays compact (10px–24px) — this is an app for scanning feeds and channels, not an editorial site.
- **Off-white always**: primary text is `#F8F8F2`, never `#FFFFFF` — pure white vibrates on violet darkness.
- **Mono as the machine voice**: anything produced by the system (health checks, timestamps, bot posts, meeting codes) renders in JetBrains Mono, visually separating human talk from machine talk.
- **Muted by default**: secondary information sits at `#7970A9`; color is earned, not decorative.

## 4. Component Stylings

### Buttons

**Primary (Purple)**
- Background: `#9580FF`
- Text: `#22212C` (dark text on neon)
- Padding: 10px 20px
- Radius: 8px
- Hover: lighten to `#A99BFF`; Focus: purple glow ring
- Use: New Post, Schedule Meeting, Register App, Save

**Secondary (Ghost)**
- Background: transparent
- Text: `#F8F8F2`
- Border: `1px solid #454158`
- Padding: 10px 20px
- Radius: 8px
- Hover: background `#2A2937`
- Use: Cancel, secondary actions, filters

**Danger**
- Background: transparent, Text: `#FF9580`, Border: `1px solid #FF9580`
- Hover: background `rgba(255,149,128,0.1)`
- Use: Delete post, remove app, leave channel

**Success / Join Voice**
- Background: `#8AFF80`, Text: `#22212C`
- Radius: 8px (or 50% circular for mic controls)
- Use: Join voice, confirm, mark resolved

**Circular Icon Button**
- Background: `#2A2937`, Icon: `#F8F8F2`
- Padding: 10px, Radius: 50%
- Hover: `#454158`
- Use: Mute/unmute, attach file, emoji, close

### Cards & Containers (posts, apps, meetings)
- Background: `#2A2937`
- Radius: 12px
- Border: none (surface contrast does the work); optional `1px solid #454158` on interactive cards
- Hover: background `#313041`
- Shadow: `rgba(0,0,0,0.3) 0px 4px 12px`
- Post cards: author avatar (50% circle, 40px) + name (16px/700) + timestamp (mono 12px `#7970A9`)

### Status Badges (Apps)
- **UP**: `#8AFF80` dot with green glow + "UP" in JetBrains Mono 11px, text `#8AFF80`
- **DOWN**: `#FF9580` dot with red glow + "DOWN", text `#FF9580`
- **PENDING**: `#FFCA80` dot, text `#FFCA80`
- Chip background: `rgba(color, 0.1)`, radius 6px, padding 2px 8px

### Inputs
- Background: `#1B1A23`
- Text: `#F8F8F2`; Placeholder: `#7970A9`
- Border: `1px solid #454158`
- Radius: 8px; Padding: 10px 14px
- Focus: border `#9580FF` + purple glow ring (`rgba(149,128,255,0.4) 0 0 0 3px`)
- Message composer: pill variant (radius 24px) pinned to channel bottom

### Navigation (Left Sidebar)
- Background: `#1B1A23`, full height, 260px wide
- Codeline logo top; nav items: Home, Calendar, Meet, Apps, Notifications
- Active item: background `#454158`, text `#F8F8F2` 700, left accent bar 3px `#9580FF`, radius 8px
- Inactive: text `#7970A9` 400; hover: background `#2A2937`
- Notification badge: `#FF80BF` circle, dark text, 11px/600
- **Profile block pinned at bottom**: rounded avatar (50%, 36px), name 14px/600 `#F8F8F2`, status 12px mono `#7970A9`, online dot `#8AFF80`

### Chat & Channels (Meet)
- Channel list on `#1B1A23`; active channel row `#454158` with radius 6px
- Private channels: lock icon in `#7970A9`; public: `#` prefix
- Messages: no bubbles — Discord-style rows on hover-highlight `#2A2937`
- Mentions: `@name` in `#FF80BF` on `rgba(255,128,191,0.1)` chip
- Bot messages: cyan left border 2px, "BOT" chip (`#80FFEA` background, `#22212C` text)
- Voice participants: avatar circles; speaking = pink glow ring pulse

### Calendar
- Grid lines: `#2A2937` hairlines on `#22212C`
- Today: cell tinted `rgba(149,128,255,0.08)`, date number in `#9580FF` 700
- Meeting events: purple chip (`rgba(149,128,255,0.15)` background, `#9580FF` text, 6px radius)
- Incident (app down) events: red chip (`rgba(255,149,128,0.15)` background, `#FF9580` text)
- Meeting links: `#80FFEA`, underline on hover

## 5. Layout Principles

### Spacing System
- Base unit: 8px
- Scale: 2px, 4px, 8px, 12px, 16px, 20px, 24px, 32px, 48px

### Grid & Container
- **Three-zone shell**: fixed left sidebar (260px, `#1B1A23`) + fluid main content (`#22212C`) + right rail (300px, DMs/contacts, on Home)
- Feed column max-width: 680px, centered in main zone
- Meet: channel list (240px) + message area (fluid) + members panel (240px, collapsible)
- Apps: responsive card grid (3 → 2 → 1 columns)

### Whitespace Philosophy
- **Comfortable density**: denser than a marketing site, airier than Discord — 16px–24px padding inside cards, 16px gaps between feed items.
- The violet-dark background provides visual rest; separation comes from surface steps (`#1B1A23` → `#22212C` → `#2A2937`), not from borders or big gaps.

### Border Radius Scale
- Subtle (6px): badges, chips, event pills, channel rows
- Standard (8px): buttons, inputs, nav items
- Comfortable (12px): cards, posts, modals, panels
- Pill (24px): message composer, search input
- Circle (50%): avatars, icon buttons, voice controls, status dots

## 6. Depth & Elevation

| Level | Surface | Treatment | Use |
|-------|---------|-----------|-----|
| Recessed (−1) | `#1B1A23` | none | Sidebars, input wells, code blocks |
| Base (0) | `#22212C` | none | Page background |
| Surface (1) | `#2A2937` | `rgba(0,0,0,0.3) 0px 4px 12px` | Cards, posts, channel panels |
| Raised (2) | `#313041` | `rgba(0,0,0,0.3) 0px 4px 12px` | Dropdowns, popovers, hover cards |
| Overlay (3) | `#2A2937` | `rgba(0,0,0,0.5) 0px 8px 24px` + backdrop `rgba(27,26,35,0.8)` | Modals, dialogs |
| Focus | any | Neon glow ring (purple/pink/green) | Focused inputs, live states |

**Elevation Philosophy**: depth is communicated primarily by **lighter violet surface steps** — the closer to the user, the lighter the violet. Shadows are supporting actors (soft, black, medium opacity); **glows are reserved for meaning** (focus, live voice, up/down status), never decoration.

## 7. Do's and Don'ts

### Do
- Keep every background in the violet-dark family (`#1B1A23`–`#454158`) — the purple tint is the identity
- Use `#F8F8F2` for primary text and `#7970A9` for everything secondary
- Assign accents by meaning: purple = action, green = up/online, red `#FF9580` = down/error, cyan = links/bots, pink = live/mentions, orange/yellow = warnings
- Put dark text (`#22212C`) on neon-filled buttons — neons are too bright for white text
- Use JetBrains Mono for timestamps, statuses, health checks and bot output
- Use surface steps for elevation and neon glows for focus/live states
- Keep avatars, voice controls and status dots perfectly circular

### Don't
- Don't use pure black (`#000000`) or pure white (`#FFFFFF`) surfaces or text — everything is tinted
- Don't use neon colors as large background fills — they are accents on 10%-opacity tints at most
- Don't mix accent meanings (a green button that deletes, a red "online" dot) — the color code is a contract
- Don't use gray borders from outside the palette — borders are `#454158` or absent
- Don't put white text on `#9580FF`/`#8AFF80` buttons — contrast comes from the dark night color
- Don't add new hues — the eight Default Theme colors are the complete vocabulary
- Don't rely on shadows alone for elevation — on violet darkness, surface lightening reads better

## 8. Responsive Behavior

### Breakpoints
| Name | Width | Key Changes |
|------|-------|-------------|
| Mobile | <640px | Single column; sidebar → bottom tab bar (Home, Calendar, Meet, Apps, Notifications); right rail hidden |
| Tablet | 640–1024px | Icon-only left sidebar (72px); right rail hidden; feed fluid |
| Desktop | 1024–1440px | Full sidebar (260px) + main; right rail on Home ≥1200px |
| Large | >1440px | Full three-zone layout; Meet members panel visible |

### Collapsing Strategy
- Left sidebar: full (260px) → icons only (72px) → bottom tab bar; profile block collapses to avatar only
- Right DM rail: fixed panel → floating chat button (bottom-right) with unread badge
- Meet: members panel and channel list become slide-over drawers on mobile
- Calendar: month grid → agenda list on mobile
- Apps grid: 3 → 2 → 1 columns
- Post composer: inline card → full-screen modal on mobile

## 9. Agent Prompt Guide

### Quick Color Reference
- Background: Night (`#22212C`)
- Recessed / sidebar: Deep Night (`#1B1A23`)
- Surface / card: `#2A2937`
- Selected / border: `#454158`
- Text: Foreground (`#F8F8F2`)
- Secondary text: Comment (`#7970A9`)
- Accent / brand: Purple (`#9580FF`)
- Success / up: Green (`#8AFF80`)
- Error / down: Red (`#FF9580`)
- Warning: Orange (`#FFCA80`) / Yellow (`#FFFF80`)
- Links / bots / info: Cyan (`#80FFEA`)
- Live / mentions / badges: Pink (`#FF80BF`)

### Example Component Prompts
- "Create a post card: `#2A2937` background, 12px radius, shadow `rgba(0,0,0,0.3) 0 4px 12px`. Author name 16px Inter 700 `#F8F8F2`, timestamp 12px JetBrains Mono `#7970A9`. Hover background `#313041`."
- "Design a primary button: `#9580FF` background, `#22212C` text, 8px radius, 10px 20px padding, 14px Inter 600, letter-spacing 0.3px. Focus ring `rgba(149,128,255,0.4) 0 0 0 3px`."
- "Build an app status badge: dot `#8AFF80` with glow `rgba(138,255,128,0.4) 0 0 8px`, label 'UP' in JetBrains Mono 11px `#8AFF80`, chip background `rgba(138,255,128,0.1)`, 6px radius."
- "Create the sidebar: `#1B1A23` background, 260px. Active item: `#454158` background, 3px `#9580FF` left bar, text `#F8F8F2` 700. Inactive: `#7970A9` 400. Profile pinned bottom: 36px circular avatar, `#8AFF80` online dot."
- "Design a bot message row: 2px `#80FFEA` left border, 'BOT' chip `#80FFEA` background with `#22212C` 10px mono 700 uppercase text, message body 16px Inter `#F8F8F2`."
- "Create a calendar incident event: chip background `rgba(255,149,128,0.15)`, text `#FF9580` 13px 600, 6px radius, meeting link in `#80FFEA`."

### Iteration Guide
1. Start every surface in the violet-dark family — `#1B1A23` recessed, `#22212C` base, `#2A2937` cards
2. Purple `#9580FF` for anything clickable-and-primary; dark text on neon fills
3. Wire the semantic contract: green up, red down, cyan bot/link, pink live, orange/yellow warn
4. Mono voice for the machine — timestamps, health checks, bot posts in JetBrains Mono
5. Elevation = lighter violet; focus/live = neon glow; shadows stay soft and black
6. Circles for people and voice (avatars, mic buttons, status dots); 8–12px radius for everything else
