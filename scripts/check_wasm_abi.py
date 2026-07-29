#!/usr/bin/env python3
"""Check that the contract ABI (exported functions) matches expected names.

Reads each contract's `lib.rs` source, extracts all `pub fn` declarations,
and asserts they match a known expected list. The Soroban `#[contractimpl]`
macro exports all `pub fn` inside the annotated `impl` block, so checking
all `pub fn` in the source verifies the contract's on-chain interface.

This catches accidental additions or removals that would change the
contract's exported ABI without updating the expected set.

Usage:
    python3 scripts/check_wasm_abi.py [--contracts-dir CONTRACTS_DIR]
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path
from typing import Dict, Set

ROOT = Path(__file__).resolve().parent.parent
CONTRACTS_DIR = ROOT / "contracts"

# ── Expected exported function names per contract ──────────────────────────
# These lists should be updated whenever a pub fn is added or removed from
# a #[contractimpl] block.
EXPECTED_FUNCTIONS: Dict[str, Set[str]] = {
    "hunty-core": {
        "initialize_admin",
        "pause_contract",
        "unpause_contract",
        "is_contract_paused",
        "create_hunt",
        "clone_hunt",
        "set_time_bonus_config",
        "update_hunt",
        "update_hunt_description",
        "set_max_players",
        "get_hunt_end_time",
        "add_clue",
        "add_clues",
        "add_clue_aliases",
        "get_clue",
        "list_clues",
        "list_hunts",
        "search_hunts",
        "set_hunt_categories",
        "get_hunts_by_category",
        "set_hunt_difficulty_override",
        "set_clue_hint",
        "request_hint",
        "list_clues_paginated",
        "activate_hunt",
        "deactivate_hunt",
        "cancel_hunt",
        "close_hunt",
        "archive_hunt",
        "get_hunt_info",
        "set_reward_manager",
        "set_admin",
        "blacklist_creator",
        "remove_from_blacklist",
        "is_blacklisted",
        "complete_hunt",
        "register_player",
        "generate_invite_code",
        "set_hunt_privacy",
        "revoke_invite_code",
        "register_with_invite",
        "preview_answer",
        "submit_answer",
        "submit_answer_with_hash",
        "get_player_progress",
        "get_completed_clues",
        "get_hunt_count",
        "get_hunt_leaderboard",
        "get_hunt_leaderboard_window",
        "get_hunt_statistics",
        "add_view_only_access",
        "remove_view_only_access",
        "is_view_only",
        "get_view_only_list",
        "add_co_creator",
        "remove_co_creator",
        "get_co_creators",
        "propose_new_admin",
        "accept_admin",
        "add_global_view_only",
        "remove_global_view_only",
        "is_global_view_only",
        "get_global_view_only_list",
        "pause_registrations",
        "unpause_registrations",
        "pause_answers",
        "unpause_answers",
        "pause_rewards",
        "unpause_rewards",
        "get_pause_state",
        "get_schema_version",
        "initialize_schema",
        "run_migration",
        "rollback_migration",
        "get_health_dashboard",
    },
    "reward-manager": {
        "initialize",
        "set_nft_reward_contract",
        "set_hunty_core",
        "add_authorized_contract",
        "remove_authorized_contract",
        "create_reward_pool_with_nft",
        "create_reward_pool",
        "update_pool_config",
        "set_pool_target_amount",
        "set_min_distribution_interval",
        "set_distribution_mode",
        "set_pool_tiers",
        "set_pool_nft_contract",
        "get_pool_config",
        "fund_reward_pool",
        "refund_pool",
        "migrate_pool",
        "get_reward_pool",
        "get_pool_statistics",
        "validate_pool",
        "freeze_pool",
        "unfreeze_pool",
        "is_pool_frozen",
        "set_daily_pool_cap",
        "set_daily_global_cap",
        "distribute_rewards",
        "distribute_batch",
        "retry_failed_nft_mint",
        "get_total_xlm_distributed",
        "distribute_rewards_legacy",
        "get_distribution_status",
        "get_dist_cooldown",
        "get_distribution_proof",
        "verify_distribution",
        "distribute_proportional",
        "get_pool_balance",
        "get_min_distribution_amount",
        "is_reward_distributed",
        "admin_resolve_distribution",
        "get_pool_distributions",
        "get_pool_distribution_count",
        "admin_withdraw_unclaimed",
        "pause",
        "unpause",
        "is_paused",
        "emergency_withdraw",
        "get_emergency_logs",
        "contract_version",
        "check_nft_reward_compatibility",
        "get_schema_version",
        "initialize_schema",
        "propose_upgrade",
        "set_upgrade_timelock",
        "get_upgrade_proposal",
        "get_upgrade_timelock",
        "get_upgrade_history",
        "run_migration",
        "rollback_migration",
        "get_health_dashboard",
        "get_pool_audit_log",
    },
    "nft-reward": {
        "initialize",
        "mint_reward_nft",
        "mint_reward_nft_from_map",
        "get_nft",
        "get_collection_metadata",
        "get_nft_metadata",
        "set_nft_extension",
        "get_nft_extension",
        "get_nft_extensions",
        "remove_nft_extension",
        "get_admin",
        "set_reward_manager",
        "add_authorized_contract",
        "remove_authorized_contract",
        "admin_update_image_uris",
        "update_nft_metadata",
        "total_supply",
        "get_total_nft_count",
        "get_max_supply",
        "set_max_supply",
        "get_remaining_supply",
        "list_all_nfts",
        "search_nfts_by_metadata",
        "transfer_nft",
        "owner_of",
        "get_nft_owner",
        "verify_ownership",
        "has_hunt_nft",
        "get_player_nfts",
        "get_nfts_by_hunt",
        "get_hunt_nft_count",
        "burn_nft",
    },
}


def extract_contractimpl_functions(file_path: Path) -> Set[str]:
    """Extract all pub fn names from #[contractimpl] blocks in a Rust source file.

    Collects ALL `pub fn` declarations in the file. Since the Soroban
    `#[contractimpl]` macro marks the primary impl block, and all `pub fn`
    inside it form the exported ABI, any `pub fn` found in the contract's
    `lib.rs` is part of the contract's on-chain interface.
    """
    text = file_path.read_text()
    functions: Set[str] = set()

    for func_match in re.finditer(r"pub\s+fn\s+(?P<name>\w+)\s*\(", text):
        functions.add(func_match.group("name"))

    return functions


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Check contract ABI signature names")
    parser.add_argument(
        "--contracts-dir",
        default=str(CONTRACTS_DIR),
        help="Path to contracts directory",
    )
    args = parser.parse_args(argv)

    contracts_dir = Path(args.contracts_dir)
    errors = 0

    for contract_name, expected in sorted(EXPECTED_FUNCTIONS.items()):
        lib_path = contracts_dir / contract_name / "src" / "lib.rs"
        if not lib_path.exists():
            print(f"SKIP: {contract_name} \u2014 lib.rs not found at {lib_path}")
            continue

        actual = extract_contractimpl_functions(lib_path)

        # Check for missing functions (in expected but not in actual)
        missing = expected - actual
        if missing:
            print(f"FAIL: {contract_name} \u2014 missing expected functions: {sorted(missing)}")
            errors += 1

        # Check for unexpected functions (in actual but not in expected)
        unexpected = actual - expected
        if unexpected:
            print(f"FAIL: {contract_name} \u2014 unexpected functions (update EXPECTED_FUNCTIONS): {sorted(unexpected)}")
            errors += 1

        if not missing and not unexpected:
            print(f"OK: {contract_name} \u2014 {len(actual)} functions match expected")

    if errors:
        print(f"\n\u274c {errors} contract(s) have ABI mismatches!")
        print("If you intentionally added/removed functions, update EXPECTED_FUNCTIONS in this script.")
        return 1

    print(f"\n\u2705 All contract ABIs match expected signatures.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
