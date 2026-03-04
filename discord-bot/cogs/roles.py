"""React-to-role cog — grants/removes roles based on emoji reactions."""

from __future__ import annotations

import json
import logging
from pathlib import Path

import discord
from discord.ext import commands

from config import ROLE_EMOJI_MAP

log = logging.getLogger("midinet.roles")

STATE_FILE = Path(__file__).resolve().parent.parent / "state.json"


class ReactRoles(commands.Cog):
    def __init__(self, bot: commands.Bot) -> None:
        self.bot = bot
        self._role_message_id: int | None = None

    def _load_message_id(self) -> int | None:
        if self._role_message_id:
            return self._role_message_id
        if STATE_FILE.exists():
            state = json.loads(STATE_FILE.read_text())
            mid = state.get("role_react_message_id")
            if mid:
                self._role_message_id = int(mid)
                return self._role_message_id
        return None

    @commands.Cog.listener()
    async def on_raw_reaction_add(self, payload: discord.RawReactionActionEvent) -> None:
        await self._handle_reaction(payload, add=True)

    @commands.Cog.listener()
    async def on_raw_reaction_remove(self, payload: discord.RawReactionActionEvent) -> None:
        await self._handle_reaction(payload, add=False)

    async def _handle_reaction(
        self, payload: discord.RawReactionActionEvent, *, add: bool
    ) -> None:
        msg_id = self._load_message_id()
        if not msg_id or payload.message_id != msg_id:
            return

        # Ignore bot reactions
        if payload.user_id == self.bot.user.id:
            return

        emoji = str(payload.emoji)
        role_name = ROLE_EMOJI_MAP.get(emoji)
        if not role_name:
            return

        guild = self.bot.get_guild(payload.guild_id)
        if not guild:
            return

        role = discord.utils.get(guild.roles, name=role_name)
        if not role:
            log.warning("Role '%s' not found in guild", role_name)
            return

        member = guild.get_member(payload.user_id)
        if not member:
            try:
                member = await guild.fetch_member(payload.user_id)
            except discord.NotFound:
                return

        try:
            if add:
                await member.add_roles(role)
                log.info("Granted '%s' to %s", role_name, member)
            else:
                await member.remove_roles(role)
                log.info("Removed '%s' from %s", role_name, member)
        except discord.Forbidden:
            log.warning("Missing permissions to modify roles for %s", member)


async def setup(bot: commands.Bot) -> None:
    await bot.add_cog(ReactRoles(bot))
