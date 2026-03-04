#!/usr/bin/env python3
"""MIDInet Discord bot — server setup + persistent community management."""

from __future__ import annotations

import logging
import os
import sys
from pathlib import Path

import discord
from discord.ext import commands
from dotenv import load_dotenv

# Ensure imports resolve from the bot directory
sys.path.insert(0, str(Path(__file__).resolve().parent))

from setup_server import run_setup

load_dotenv(Path(__file__).resolve().parent / ".env")

TOKEN = os.getenv("DISCORD_TOKEN")
GUILD_ID = os.getenv("GUILD_ID")

if not TOKEN:
    sys.exit("DISCORD_TOKEN not set — copy .env.example to .env and fill it in.")
if not GUILD_ID:
    sys.exit("GUILD_ID not set — copy .env.example to .env and fill it in.")

GUILD_ID = int(GUILD_ID)

# ── Logging ──────────────────────────────────────────────────────────────────

logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s  %(name)-22s  %(levelname)-8s  %(message)s",
    datefmt="%H:%M:%S",
)
log = logging.getLogger("midinet.bot")

# ── Bot setup ────────────────────────────────────────────────────────────────

intents = discord.Intents.default()
intents.members = True
intents.message_content = True

bot = commands.Bot(command_prefix="!", intents=intents)


@bot.event
async def on_ready() -> None:
    log.info("Logged in as %s (id=%s)", bot.user, bot.user.id)

    guild = bot.get_guild(GUILD_ID)
    if not guild:
        log.error("Guild %s not found — is the bot invited?", GUILD_ID)
        return

    # Run one-shot setup (idempotent)
    await run_setup(guild)

    # Load cogs
    for cog in ("cogs.welcome", "cogs.roles"):
        try:
            await bot.load_extension(cog)
            log.info("Loaded cog: %s", cog)
        except commands.ExtensionAlreadyLoaded:
            pass

    log.info("MIDInet bot is ready.")


def main() -> None:
    bot.run(TOKEN, log_handler=None)


if __name__ == "__main__":
    main()
