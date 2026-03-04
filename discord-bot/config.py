"""MIDInet Discord server configuration — channels, roles, and messages."""

import discord

# ── Roles ────────────────────────────────────────────────────────────────────

ROLES = [
    {
        "name": "Developer",
        "color": discord.Colour.blue(),
        "hoist": True,
        "mentionable": True,
    },
    {
        "name": "Beta Tester",
        "color": discord.Colour.green(),
        "hoist": True,
        "mentionable": True,
    },
    {
        "name": "Community",
        "color": discord.Colour.greyple(),
        "hoist": False,
        "mentionable": False,
    },
]

# Emoji → role name mapping for react-to-role
ROLE_EMOJI_MAP = {
    "\U0001f527": "Developer",       # 🔧
    "\U0001f9ea": "Beta Tester",     # 🧪
}

# ── Categories & Channels ────────────────────────────────────────────────────

# Each category maps to a list of channel dicts.
# "read_only" = only admins/bot can send.  "restrict_to" = list of role names.
CATEGORIES = {
    "\U0001f4e2 Information": [
        {"name": "announcements", "topic": "Release notes and important updates", "read_only": True},
        {"name": "rules", "topic": "Community guidelines", "read_only": True},
        {"name": "roles", "topic": "React to pick your roles", "read_only": True},
        {"name": "roadmap", "topic": "Planned features and milestones", "read_only": True},
    ],
    "\U0001f4ac Community": [
        {"name": "general", "topic": "Main chat"},
        {"name": "introductions", "topic": "Say hi and tell us about your setup"},
        {"name": "show-and-tell", "topic": "Share your MIDInet setups and projects"},
    ],
    "\U0001f6e0 Support": [
        {"name": "setup-help", "topic": "Installation and configuration help"},
        {"name": "bug-reports", "topic": "Report bugs — include OS, version, and steps to reproduce"},
        {"name": "feature-requests", "topic": "Suggest new features"},
    ],
    "\U0001f469\u200d\U0001f4bb Development": [
        {"name": "dev-discussion", "topic": "Architecture, PRs, and technical discussion", "restrict_to": ["Developer"]},
        {"name": "github-feed", "topic": "Automated GitHub activity feed", "read_only": True},
        {"name": "beta-testing", "topic": "Pre-release builds and feedback", "restrict_to": ["Beta Tester", "Developer"]},
    ],
    "\U0001f3b9 Use Cases": [
        {"name": "resolume", "topic": "Resolume Arena integration"},
        {"name": "live-performance", "topic": "Live MIDI performance setups"},
        {"name": "raspberry-pi", "topic": "Raspberry Pi host hardware discussion"},
    ],
}

# ── Messages ─────────────────────────────────────────────────────────────────

RULES_MESSAGE = """\
**MIDInet Community Rules**

1. **Be respectful** — treat everyone with courtesy. No harassment, hate speech, or personal attacks.
2. **Stay on topic** — use the appropriate channel for your message.
3. **No spam** — no self-promotion, unsolicited DMs, or repeated messages.
4. **Search before asking** — check pins and previous messages before posting a question.
5. **Bug reports need details** — include your OS, MIDInet version, and steps to reproduce.
6. **No piracy** — don't share or request pirated software (Resolume, etc.).
7. **English preferred** — so everyone can follow along.

Breaking these rules may result in a warning, mute, or ban at moderator discretion.
"""

ROLE_REACT_MESSAGE = """\
**Pick Your Roles**

React to this message to get access to the right channels:

\U0001f527 — **Developer** (access to #dev-discussion)
\U0001f9ea — **Beta Tester** (access to #beta-testing)
"""

WELCOME_MESSAGE = """\
Welcome to **MIDInet**, {member}!

MIDInet is a real-time MIDI-over-network system with redundant failover — stream MIDI from a Raspberry Pi to any number of clients with zero configuration.

**Getting Started**
\u2022 Read <#{rules_channel_id}> for community guidelines
\u2022 Introduce yourself in <#{intro_channel_id}>
\u2022 Pick your roles in <#{roles_channel_id}>
\u2022 Need help? Head to <#{help_channel_id}>

Enjoy your stay!
"""

FIRST_ANNOUNCEMENT = """\
**MIDInet Discord is live!**

Welcome to the official MIDInet community. This is the place for:
\u2022 Release announcements and changelogs
\u2022 Setup help and troubleshooting
\u2022 Feature discussions and requests
\u2022 Sharing your MIDI-over-network setups

The project is in active development. If you're interested in beta testing, grab the **Beta Tester** role in <#{roles_channel_id}> and join <#{beta_channel_id}>.

Thanks for being here early — your feedback shapes the project.
"""
