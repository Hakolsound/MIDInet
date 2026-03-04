"""Welcome cog — greets new members and auto-assigns the Community role."""

from __future__ import annotations

import json
import logging
from pathlib import Path

import discord
from discord.ext import commands

from config import WELCOME_MESSAGE

log = logging.getLogger("midinet.welcome")

STATE_FILE = Path(__file__).resolve().parent.parent / "state.json"


class Welcome(commands.Cog):
    def __init__(self, bot: commands.Bot) -> None:
        self.bot = bot

    def _channel_ids(self) -> dict:
        if STATE_FILE.exists():
            state = json.loads(STATE_FILE.read_text())
            return state.get("channels", {})
        return {}

    @commands.Cog.listener()
    async def on_member_join(self, member: discord.Member) -> None:
        guild = member.guild

        # Auto-assign Community role
        community_role = discord.utils.get(guild.roles, name="Community")
        if community_role:
            try:
                await member.add_roles(community_role)
                log.info("Assigned 'Community' role to %s", member)
            except discord.Forbidden:
                log.warning("Missing permissions to assign role to %s", member)

        # Send welcome message in #general
        ids = self._channel_ids()
        general_id = ids.get("general")
        if not general_id:
            log.warning("No 'general' channel ID in state — skipping welcome message")
            return

        channel = guild.get_channel(general_id)
        if not channel:
            log.warning("Could not find general channel (id=%s)", general_id)
            return

        text = WELCOME_MESSAGE.format(
            member=member.mention,
            rules_channel_id=ids.get("rules", ""),
            intro_channel_id=ids.get("introductions", ""),
            roles_channel_id=ids.get("roles", ""),
            help_channel_id=ids.get("setup-help", ""),
        )
        await channel.send(text)
        log.info("Sent welcome message for %s", member)


async def setup(bot: commands.Bot) -> None:
    await bot.add_cog(Welcome(bot))
