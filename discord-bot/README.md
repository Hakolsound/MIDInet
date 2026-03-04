# MIDInet Discord Bot

Automated server setup + persistent community bot for the MIDInet Discord.

## What it does

**First run:** Creates the entire server structure — roles, categories, channels, permissions, and initial messages (rules, role-react, first announcement).

**Ongoing:** Stays running to handle:
- Welcome messages for new members (posted in #general)
- Auto-assign `@Community` role on join
- React-to-role in #roles (🔧 Developer, 🧪 Beta Tester)

## Setup

### 1. Create the Discord server

Open Discord, click **+**, and create a new server named **MIDInet**.

### 2. Create a bot application

1. Go to https://discord.com/developers/applications
2. Click **New Application** → name it **MIDInet Bot**
3. Go to **Bot** tab → click **Reset Token** → copy the token
4. Enable these **Privileged Gateway Intents**:
   - Server Members Intent
   - Message Content Intent
5. Go to **OAuth2** → **URL Generator**:
   - Scopes: `bot`
   - Bot Permissions: `Administrator`
6. Copy the generated URL, open it in your browser, and invite the bot to your server

### 3. Get your Guild ID

1. In Discord, go to **Settings → Advanced → Developer Mode** (turn on)
2. Right-click your server name → **Copy Server ID**

### 4. Configure and run

```bash
cd discord-bot
cp .env.example .env
# Edit .env — paste your bot token and guild ID

pip install -r requirements.txt
python bot.py
```

The bot will:
1. Create all roles, categories, and channels
2. Post rules, role-react message, and first announcement
3. Start listening for new members and reactions

### Re-running setup

Setup is idempotent — it skips anything that already exists. To force a full re-run, delete `state.json` and run the bot again.

## Project structure

```
discord-bot/
├── bot.py              # Entry point
├── setup_server.py     # One-shot server setup
├── config.py           # All server structure + messages
├── cogs/
│   ├── welcome.py      # Welcome message + auto-role on join
│   └── roles.py        # React-to-role system
├── requirements.txt
├── .env.example
└── .gitignore
```

## Customization

Edit `config.py` to change:
- Channel names, topics, and permissions
- Role names, colors, and emoji mappings
- All message text (rules, welcome, announcements)
