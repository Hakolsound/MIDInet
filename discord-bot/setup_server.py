"""One-shot server setup — creates roles, categories, channels, and posts initial messages.

Idempotent: skips anything that already exists.
"""

from __future__ import annotations

import json
import logging
from pathlib import Path

import discord

from config import (
    CATEGORIES,
    FIRST_ANNOUNCEMENT,
    ROLE_EMOJI_MAP,
    ROLE_REACT_MESSAGE,
    ROLES,
    RULES_MESSAGE,
)

log = logging.getLogger("midinet.setup")

STATE_FILE = Path(__file__).parent / "state.json"


def _load_state() -> dict:
    if STATE_FILE.exists():
        return json.loads(STATE_FILE.read_text())
    return {}


def _save_state(state: dict) -> None:
    STATE_FILE.write_text(json.dumps(state, indent=2))


async def _get_or_create_role(guild: discord.Guild, role_cfg: dict) -> discord.Role:
    """Return an existing role by name, or create it."""
    existing = discord.utils.get(guild.roles, name=role_cfg["name"])
    if existing:
        log.info("Role '%s' already exists — skipping", role_cfg["name"])
        return existing

    role = await guild.create_role(
        name=role_cfg["name"],
        colour=role_cfg["color"],
        hoist=role_cfg["hoist"],
        mentionable=role_cfg["mentionable"],
    )
    log.info("Created role '%s'", role.name)
    return role


async def _get_or_create_category(
    guild: discord.Guild, name: str
) -> discord.CategoryChannel:
    existing = discord.utils.get(guild.categories, name=name)
    if existing:
        log.info("Category '%s' already exists — skipping", name)
        return existing

    category = await guild.create_category(name)
    log.info("Created category '%s'", name)
    return category


async def _get_or_create_channel(
    guild: discord.Guild,
    category: discord.CategoryChannel,
    ch_cfg: dict,
    roles: dict[str, discord.Role],
) -> discord.TextChannel:
    existing = discord.utils.get(guild.text_channels, name=ch_cfg["name"], category=category)
    if existing:
        log.info("Channel '#%s' already exists — skipping", ch_cfg["name"])
        return existing

    overwrites: dict[discord.Role | discord.Member, discord.PermissionOverwrite] = {}

    if ch_cfg.get("read_only"):
        # Everyone can read but not send
        overwrites[guild.default_role] = discord.PermissionOverwrite(send_messages=False)

    if ch_cfg.get("restrict_to"):
        # Hide from everyone, show to listed roles
        overwrites[guild.default_role] = discord.PermissionOverwrite(view_channel=False)
        for role_name in ch_cfg["restrict_to"]:
            role = roles.get(role_name)
            if role:
                overwrites[role] = discord.PermissionOverwrite(view_channel=True)

    channel = await guild.create_text_channel(
        name=ch_cfg["name"],
        category=category,
        topic=ch_cfg.get("topic"),
        overwrites=overwrites,
    )
    log.info("Created channel '#%s'", channel.name)
    return channel


async def run_setup(guild: discord.Guild) -> dict[str, int]:
    """Run full server setup. Returns a dict of notable channel IDs."""
    state = _load_state()

    if state.get("setup_complete"):
        log.info("Setup already completed — skipping (delete state.json to re-run)")
        return state.get("channels", {})

    # ── Roles ────────────────────────────────────────────────────────────
    roles: dict[str, discord.Role] = {}
    for role_cfg in ROLES:
        roles[role_cfg["name"]] = await _get_or_create_role(guild, role_cfg)

    # ── Categories & Channels ────────────────────────────────────────────
    channels: dict[str, discord.TextChannel] = {}
    for cat_name, ch_list in CATEGORIES.items():
        category = await _get_or_create_category(guild, cat_name)
        for ch_cfg in ch_list:
            ch = await _get_or_create_channel(guild, category, ch_cfg, roles)
            channels[ch_cfg["name"]] = ch

    # ── Post messages ────────────────────────────────────────────────────
    channel_ids = {name: ch.id for name, ch in channels.items()}

    # Rules
    rules_ch = channels.get("rules")
    if rules_ch:
        await rules_ch.send(RULES_MESSAGE)
        log.info("Posted rules message")

    # Role-react message
    roles_ch = channels.get("roles")
    if roles_ch:
        msg = await roles_ch.send(ROLE_REACT_MESSAGE)
        for emoji in ROLE_EMOJI_MAP:
            await msg.add_reaction(emoji)
        channel_ids["role_react_message_id"] = msg.id
        log.info("Posted role-react message (id=%s)", msg.id)

    # First announcement
    ann_ch = channels.get("announcements")
    if ann_ch:
        roles_channel_id = channel_ids.get("roles", "")
        beta_channel_id = channel_ids.get("beta-testing", "")
        text = FIRST_ANNOUNCEMENT.format(
            roles_channel_id=roles_channel_id,
            beta_channel_id=beta_channel_id,
        )
        await ann_ch.send(text)
        log.info("Posted first announcement")

    # ── Persist state ────────────────────────────────────────────────────
    state = {
        "setup_complete": True,
        "channels": channel_ids,
        "role_react_message_id": channel_ids.get("role_react_message_id"),
    }
    _save_state(state)
    log.info("Setup complete — state saved to %s", STATE_FILE)

    return channel_ids
