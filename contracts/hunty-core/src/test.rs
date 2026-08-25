use crate::HuntyCore;
use soroban_sdk::testutils::{Address as _, Ledger as _};
use soroban_sdk::{Address, Env, String};

/// Helper to execute contract operations within the contract context.
/// Wraps calls with `env.as_contract()` for proper storage isolation.
fn execute_in_contract<T, F>(env: &Env, contract_id: &Address, f: F) -> T
where
    F: FnOnce(&Env) -> T,
{
    env.as_contract(contract_id, || f(env))
}
#[cfg(test)]
extern crate std;

use std::string::ToString;

#[cfg(test)]
mod test {
    // Benchmark-style micro tests (best-effort gas/footprint proxy)

    use super::*;
    use crate::ANSWER_SUBMISSION_WINDOW_SECS;
    use soroban_sdk::{Address, Env, IntoVal, String, Symbol, TryIntoVal, Vec};
    // Bring Soroban testutils traits into scope (generate addresses, set ledger info, register contracts).
    use crate::errors::{HuntError, HuntErrorCode};
    use crate::storage::Storage;
    use crate::types::{
        BatchClueInput, ClueAddedEvent, ClueInfo, CreatorBlacklistedEvent, CreatorRemovedFromBlacklistEvent,
        HuntCancelledEvent, HuntClosedEvent, HuntCompletedEvent, HuntCreatedEvent, HuntStatus,
        HuntStatusChangedEvent, LeaderboardResult, PlayerRegisteredEvent, RewardClaimFailedEvent,
        TimeBonusConfig,
    };
    use crate::HuntyCore;
    use nft_reward::NftReward;
    use reward_manager::RewardManager;
    use soroban_sdk::testutils::{Address as _, Events as _, Ledger as _, Register as _};
    use soroban_sdk::{token, String as SorobanString, TryFromVal, Val};

    /// Runs a closure inside a registered HuntyCore contract context so storage is accessible.
    fn with_core_contract<T>(env: &Env, f: impl FnOnce(&Env, &Address) -> T) -> T {
        let contract_id = env.register_contract(None, super::HuntyCore);
        env.as_contract(&contract_id, || f(env, &contract_id))
    }

    fn find_hunt_status_changed_event(env: &Env) -> Option<HuntStatusChangedEvent> {
        let expected_topic = Symbol::new(env, "HuntStatusChanged").into_val(env);
        let events = env.events().all();
        let mut idx = 0;
        while idx < events.len() {
            let event = events.get(idx).unwrap();
            let topics = &event.1;
            if topics.len() > 0 {
                let topic = topics.get(0).unwrap();
                if *topic == expected_topic {
                    return HuntStatusChangedEvent::try_from_val(env, &event.2).ok();
                }
            }
            idx += 1;
        }
        None
    }

    fn find_event<T: TryFromVal<Env, Val>>(env: &Env, topic_name: &str) -> Option<(Vec<Val>, T)> {
        let expected_topic = Symbol::new(env, topic_name).into_val(env);
        let events = env.events().all();
        let mut idx = 0;
        while idx < events.len() {
            let event = events.get(idx).unwrap();
            let topics = event.1.clone();
            if topics.len() > 0 && topics.get(0).unwrap() == expected_topic {
                if let Ok(data) = T::try_from_val(env, &event.2) {
                    return Some((topics, data));
                }
            }
            idx += 1;
        }
        None
    }

    /// Runs a closure in the given contract's context. Use when multiple invocations must share
    /// the same storage; call once per step that uses require_auth (Soroban allows one auth per frame).
    fn as_core_contract<T>(env: &Env, contract_id: &Address, f: impl FnOnce(&Env) -> T) -> T {
        env.as_contract(contract_id, || f(env))
    }

    /// Helper to set up RewardManager with XLM token and optional default NFT contract.
    fn setup_reward_manager(
        env: &Env,
        nft_contract: Option<&Address>,
    ) -> (Address, Address, Address) {
        let reward_manager_id = env.register(RewardManager, ());
        let token_admin = Address::generate(env);
        let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
        let token_address = token_contract.address();

        env.as_contract(&reward_manager_id, || {
            RewardManager::initialize(env.clone(), token_admin.clone(), token_address.clone())
                .unwrap();
        });
        if let Some(nft) = nft_contract {
            env.mock_all_auths();
            env.as_contract(&reward_manager_id, || {
                RewardManager::set_nft_reward_contract(
                    env.clone(),
                    token_admin.clone(),
                    nft.clone(),
                )
                .unwrap();
            });
        }

        (reward_manager_id, token_address, token_admin)
    }

    fn submit_answer(
        env: &Env,
        hunt_id: u64,
        clue_id: u32,
        player: Address,
        answer: String,
        nonce: u64,
    ) -> Result<bool, HuntErrorCode> {
        let now = env.ledger().timestamp();
        HuntyCore::submit_answer(env.clone(), hunt_id, clue_id, player, answer, nonce, now)
    }

    #[test]
    fn test_error_with_context_display() {
        let err = HuntError::HuntNotFound;
        let hunt_error: HuntErrorCode = err.into();
        assert_eq!(hunt_error, HuntErrorCode::HuntNotFound)
    }

    #[test]
    fn test_all_error_codes_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        let variants: &[(HuntErrorCode, &str)] = &[
            (HuntErrorCode::HuntNotFound, "HuntNotFound"),
            (HuntErrorCode::ClueNotFound, "ClueNotFound"),
            (HuntErrorCode::InvalidHuntStatus, "InvalidHuntStatus"),
            (HuntErrorCode::PlayerNotRegistered, "PlayerNotRegistered"),
            (HuntErrorCode::ClueAlreadyCompleted, "ClueAlreadyCompleted"),
            (HuntErrorCode::InvalidAnswer, "InvalidAnswer"),
            (HuntErrorCode::HuntNotActive, "HuntNotActive"),
            (HuntErrorCode::Unauthorized, "Unauthorized"),
            (HuntErrorCode::InsufficientRewardPool, "InsufficientRewardPool"),
            (HuntErrorCode::DuplicateRegistration, "DuplicateRegistration"),
            (HuntErrorCode::InvalidTitle, "InvalidTitle"),
            (HuntErrorCode::InvalidDescription, "InvalidDescription"),
            (HuntErrorCode::InvalidAddress, "InvalidAddress"),
            (HuntErrorCode::TooManyClues, "TooManyClues"),
            (HuntErrorCode::InvalidQuestion, "InvalidQuestion"),
            (HuntErrorCode::RefundFailed, "RefundFailed"),
            (HuntErrorCode::NoCluesAdded, "NoCluesAdded"),
            (HuntErrorCode::HuntNotCompleted, "HuntNotCompleted"),
            (HuntErrorCode::RewardAlreadyClaimed, "RewardAlreadyClaimed"),
            (HuntErrorCode::RewardDistributionFailed, "RewardDistributionFailed"),
            (HuntErrorCode::NoRewardsConfigured, "NoRewardsConfigured"),
            (HuntErrorCode::DuplicateSubmission, "DuplicateSubmission"),
            (HuntErrorCode::SubmissionExpired, "SubmissionExpired"),
            (HuntErrorCode::BannedPlayer, "BannedPlayer"),
            (HuntErrorCode::NoRequiredClues, "NoRequiredClues"),
            (HuntErrorCode::RateLimitExceeded, "RateLimitExceeded"),
            (HuntErrorCode::ScoreOverflow, "ScoreOverflow"),
            (HuntErrorCode::RegistrationsPaused, "RegistrationsPaused"),
            (HuntErrorCode::AnswersPaused, "AnswersPaused"),
            (HuntErrorCode::RewardsPaused, "RewardsPaused"),
            (HuntErrorCode::HuntEndTimeInPast, "HuntEndTimeInPast"),
            (HuntErrorCode::NoPendingAdmin, "NoPendingAdmin"),
            (HuntErrorCode::PendingAdminMismatch, "PendingAdminMismatch"),
            (HuntErrorCode::InvalidRarity, "InvalidRarity"),
            (HuntErrorCode::InvalidTimeBonusConfig, "InvalidTimeBonusConfig"),
            (HuntErrorCode::AddressBlacklisted, "AddressBlacklisted"),
            (HuntErrorCode::ContractPaused, "ContractPaused"),
        ];
        for (variant, name) in variants {
            let code = *variant as u32;
            assert!(
                seen.insert(code),
                "Duplicate HuntErrorCode value {} for variant '{}'",
                code,
                name
            );
        }
    }

    #[test]
    fn test_hunt_not_found_converts_to_code() {
        let err = HuntError::HuntNotFound;
        let code: HuntErrorCode = err.into();
        assert_eq!(code, HuntErrorCode::HuntNotFound);
    }

    #[test]
    fn test_issue_686_error_variants_convert_to_codes() {
        let cases = [
            (HuntError::RefundFailed, HuntErrorCode::RefundFailed),
            (HuntError::NoCluesAdded, HuntErrorCode::NoCluesAdded),
            (
                HuntError::InvalidMaxAttempts,
                HuntErrorCode::InvalidMaxAttempts,
            ),
        ];

        for (error, expected_code) in cases {
            let code: HuntErrorCode = error.into();
            assert_eq!(code, expected_code);
        }
    }

    #[test]
    fn test_clue_not_found_converts_to_code() {
        let err = HuntError::ClueNotFound;
        let code: HuntErrorCode = err.into();
        assert_eq!(code, HuntErrorCode::ClueNotFound);
    }

    #[test]
    fn test_submit_answer_with_hash_works() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let player1 = Address::generate(&env);
        let player2 = Address::generate(&env);
        let contract_id = env.register(HuntyCore, ());

        // Create hunt
        env.mock_all_auths();
        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hash Hunt"),
                String::from_str(env, "Test hashing paths"),
                None,
                None,
                0,
                None,
            )
        })
        .unwrap();

        // Add a clue with answer "Paris"
        env.mock_all_auths();
        let clue_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(
                env.clone(),
                hunt_id,
                String::from_str(env, "Capital of France?"),
                String::from_str(env, "Paris"),
                10,
                true,
                None,
            )
        })
        .unwrap();

        // Register two players
        env.as_contract(&contract_id, || {
            HuntyCore::register_player(env.clone(), hunt_id, player1.clone()).unwrap();
        });
        env.as_contract(&contract_id, || {
            HuntyCore::register_player(env.clone(), hunt_id, player2.clone()).unwrap();
        });

        // Submit plaintext answer for player1
        let res1 = env.as_contract(&contract_id, || {
            HuntyCore::submit_answer(
                env.clone(),
                hunt_id,
                clue_id,
                player1.clone(),
                String::from_str(&env, "Paris"),
                1,
                env.ledger().timestamp(),
            )
        });
        assert!(res1.is_ok());

        // Compute precomputed hash (uses same normalization helper) and submit for player2
        let pre_hash = HuntyCore::normalize_and_hash_answer(&env, hunt_id, clue_id, &String::from_str(&env, "Paris")).unwrap();
        let res2 = env.as_contract(&contract_id, || {
            HuntyCore::submit_answer_with_hash(
                env.clone(),
                hunt_id,
                clue_id,
                player2.clone(),
                pre_hash.clone(),
                1,
                env.ledger().timestamp(),
            )
        });
        assert!(res2.is_ok());
    }

    #[test]
    fn test_hunt_completion_ranks() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let player1 = Address::generate(&env);
        let player2 = Address::generate(&env);
        let player3 = Address::generate(&env);
        let contract_id = env.register(HuntyCore, ());

        // Create hunt
        env.mock_all_auths();
        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Rank Hunt"),
                String::from_str(env, "Test ranking"),
                None,
                None,
                0,
                None,
            )
        })
        .unwrap();

        let question = String::from_str(&env, "What is 2+2?");
        let answer = String::from_str(&env, "4");
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(env.clone(), hunt_id, question.clone(), answer.clone(), 10, true, None).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player1.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player2.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player3.clone()).unwrap();
        });

        // Player1 completes
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            submit_answer(env, hunt_id, 1, player1.clone(), answer.clone(), 1)
            .unwrap();
        });
        let board1 = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_hunt_leaderboard(env.clone(), hunt_id, 10).unwrap().entries
        });
        let first = board1.get(0).unwrap();
        assert_eq!(first.player, player1);
        assert_eq!(first.rank, 1);
        assert!(first.is_completed);

        // Player2 completes
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            submit_answer(env, hunt_id, 1, player2.clone(), answer.clone(), 2)
            .unwrap();
        });
        let board2 = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_hunt_leaderboard(env.clone(), hunt_id, 10).unwrap().entries
        });
        let first_after_second = board2.get(0).unwrap();
        let second_after_second = board2.get(1).unwrap();
        assert_eq!(first_after_second.player, player1);
        assert_eq!(first_after_second.rank, 1);
        assert_eq!(second_after_second.player, player2);
        assert_eq!(second_after_second.rank, 2);
        assert!(second_after_second.is_completed);

        // Duplicate attempt by Player2 (should not emit new event)
        env.mock_all_auths();
        let dup_result = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(
                env.clone(),
                hunt_id,
                1,
                player2.clone(),
                answer.clone(),
                2,
                env.ledger().timestamp(),
            )
        });
        assert_eq!(dup_result, Err(HuntErrorCode::DuplicateSubmission));
        let board_dup = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_hunt_leaderboard(env.clone(), hunt_id, 10).unwrap().entries
        });
        let first_after_dup = board_dup.get(0).unwrap();
        let second_after_dup = board_dup.get(1).unwrap();
        assert_eq!(first_after_dup.player, player1);
        assert_eq!(first_after_dup.rank, 1);
        assert_eq!(second_after_dup.player, player2);
        assert_eq!(second_after_dup.rank, 2);
    }

    #[test]
    fn test_submit_answer_rejects_expired_submission_timestamp() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "What is 2+2?");
        let answer = String::from_str(&env, "4");

        let contract_id = env.register(HuntyCore, ());
        env.mock_all_auths();
        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Replay Hunt"),
                String::from_str(env, "Replay protection"),
                None,
                None,
                0,
                None,
            )
            .unwrap()
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(env.clone(), hunt_id, question.clone(), answer.clone(), 10, true, None)
                .unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            let result = HuntyCore::submit_answer(
                env.clone(),
                hunt_id,
                1,
                player.clone(),
                answer.clone(),
                1,
                env.ledger().timestamp() - ANSWER_SUBMISSION_WINDOW_SECS - 1,
            );
            assert_eq!(result, Err(HuntErrorCode::SubmissionExpired));
        });
    }

    #[test]
    fn test_hunt_created_event_topics_and_data() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Indexed Hunt");

        with_core_contract(&env, |env, _cid| {
            env.mock_all_auths();
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title.clone(),
                String::from_str(env, "Event payload coverage"),
                None,
                None,
                0,
                None,
            )
            .unwrap();

            let (topics, event) =
                find_event::<HuntCreatedEvent>(env, "HuntCreated").expect("missing HuntCreated");
            assert_eq!(topics.len(), 2);
            assert_eq!(topics.get(0).unwrap(), Symbol::new(env, "HuntCreated").into_val(env));
            assert_eq!(topics.get(1).unwrap(), hunt_id.into_val(env));
            assert_eq!(event.hunt_id, hunt_id);
            assert_eq!(event.creator, creator);
            assert_eq!(event.title, title);
        });
    }

    #[test]
    fn test_clue_added_event_topics_and_data() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let question = String::from_str(&env, "What walks on four legs?");

        let contract_id = env.register(HuntyCore, ());
        env.mock_all_auths();
        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Clue Event Hunt"),
                String::from_str(env, "Verifies indexed clue metadata"),
                None,
                None,
                0,
                None,
            )
            .unwrap()
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            let clue_id = HuntyCore::add_clue(
                env.clone(),
                hunt_id,
                question.clone(),
                String::from_str(env, "Human"),
                25,
                true,
                Some(3),
            )
            .unwrap();

            let (topics, event) =
                find_event::<ClueAddedEvent>(env, "ClueAdded").expect("missing ClueAdded");
            assert_eq!(topics.len(), 3);
            assert_eq!(topics.get(0).unwrap(), Symbol::new(env, "ClueAdded").into_val(env));
            assert_eq!(topics.get(1).unwrap(), hunt_id.into_val(env));
            assert_eq!(topics.get(2).unwrap(), clue_id.into_val(env));
            assert_eq!(event.hunt_id, hunt_id);
            assert_eq!(event.clue_id, clue_id);
            assert_eq!(event.creator, creator);
            assert_eq!(event.question, question);
            assert_eq!(event.points, 25);
            assert!(event.is_required);
        });
    }

    #[test]
    fn test_player_registered_event_topics_and_data() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let player = Address::generate(&env);

        let contract_id = env.register(HuntyCore, ());
        env.mock_all_auths();
        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Registration Event Hunt"),
                String::from_str(env, "Verifies player registration indexing"),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(
                env.clone(),
                hunt_id,
                String::from_str(env, "Q"),
                String::from_str(env, "A"),
                10,
                true,
                None,
            )
            .unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            hunt_id
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();

            let (topics, event) = find_event::<PlayerRegisteredEvent>(env, "PlayerRegistered")
                .expect("missing PlayerRegistered");
            assert_eq!(topics.len(), 2);
            assert_eq!(
                topics.get(0).unwrap(),
                Symbol::new(env, "PlayerRegistered").into_val(env)
            );
            assert_eq!(topics.get(1).unwrap(), hunt_id.into_val(env));
            assert_eq!(event.hunt_id, hunt_id);
            assert_eq!(event.player, player);
        });
    }

    #[test]
    fn test_processed_submission_tracking_expires_after_window() {
        let env = Env::default();
        let start_time = 1_700_000_000;
        env.ledger().set_timestamp(start_time);
        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "What is 2+2?");
        let answer = String::from_str(&env, "4");

        let contract_id = env.register(HuntyCore, ());
        env.mock_all_auths();
        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Replay Hunt"),
                String::from_str(env, "Replay protection"),
                None,
                None,
                0,
                None,
            )
            .unwrap()
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(env.clone(), hunt_id, question.clone(), answer.clone(), 10, true, None)
                .unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            let submitted_at = env.ledger().timestamp();
            HuntyCore::submit_answer(
                env.clone(),
                hunt_id,
                1,
                player.clone(),
                answer.clone(),
                7,
                submitted_at,
            )
            .unwrap();

            assert_eq!(
                Storage::get_processed_submission_expiry(
                    env,
                    hunt_id,
                    1,
                    &player,
                    7,
                    submitted_at,
                ),
                Some(submitted_at + ANSWER_SUBMISSION_WINDOW_SECS)
            );

            env.ledger()
                .set_timestamp(submitted_at + ANSWER_SUBMISSION_WINDOW_SECS + 1);
            HuntyCore::assert_submission_not_replayed(
                env,
                hunt_id,
                1,
                &player,
                7,
                submitted_at,
                env.ledger().timestamp(),
            )
            .unwrap();

            assert_eq!(
                Storage::get_processed_submission_expiry(
                    env,
                    hunt_id,
                    1,
                    &player,
                    7,
                    submitted_at,
                ),
                None
            );
        });
    }

    #[test]
    fn test_invalid_hunt_status_message() {
        let err = HuntError::InvalidHuntStatus;
        assert_eq!(err.to_string(), "Invalid hunt status");
    }

    #[test]
    fn test_insufficient_reward_pool_converts_to_code() {
        let err = HuntError::InsufficientRewardPool;
        let code: HuntErrorCode = err.into();
        assert_eq!(code, HuntErrorCode::InsufficientRewardPool);
    }

    // ========== create_hunt() Tests ==========

    #[test]
    fn test_create_hunt_success() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Test Hunt");
        let description = String::from_str(&env, "This is a test hunt description");

        let (hunt_id, hunt) = with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title.clone(),
                description.clone(),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            let hunt = Storage::get_hunt(env, hunt_id).unwrap();
            (hunt_id, hunt)
        });

        // Verify hunt ID is 1 (first hunt)
        assert_eq!(hunt_id, 1);
        assert_eq!(hunt.hunt_id, hunt_id);
        assert_eq!(hunt.creator, creator);
        assert_eq!(hunt.title, title);
        assert_eq!(hunt.description, description);
        assert_eq!(hunt.status, HuntStatus::Draft);
        assert_eq!(hunt.total_clues, 0);
        assert_eq!(hunt.required_clues, 0);
        assert_eq!(hunt.reward_config.xlm_pool, 0);
        assert_eq!(hunt.reward_config.nft_enabled, false);
        assert_eq!(hunt.reward_config.max_winners, 0);
        assert_eq!(hunt.reward_config.claimed_count, 0);
        assert_eq!(hunt.time_bonus_config(), None);
        assert!(hunt.created_at > 0);
        assert_eq!(hunt.activated_at, 0);
        assert_eq!(hunt.end_time, 0);
    }

    #[test]
    fn test_time_bonus_scoring_decreases_over_time() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);

        let creator = Address::generate(&env);
        let player_fast = Address::generate(&env);
        let player_mid = Address::generate(&env);
        let player_slow = Address::generate(&env);
        let title = String::from_str(&env, "Time Bonus Hunt");
        let description = String::from_str(&env, "A hunt with a decaying score bonus");
        let question = String::from_str(&env, "What time is it?");
        let answer = String::from_str(&env, "now");
        let bonus = TimeBonusConfig {
            start_multiplier_bps: 20_000,
            min_multiplier_bps: 10_000,
            decay_duration_secs: 100,
        };

        let contract_id = env.register_contract(None, super::HuntyCore);
        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title.clone(),
                description.clone(),
                None,
                None,
            )
            .unwrap()
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::set_time_bonus_config(
                env.clone(),
                hunt_id,
                creator.clone(),
                Some(bonus.clone()),
            )
            .unwrap();
            let hunt = Storage::get_hunt(env, hunt_id).unwrap();
            assert_eq!(hunt.time_bonus_config(), Some(bonus.clone()));
            HuntyCore::add_clue(
                env.clone(),
                hunt_id,
                question.clone(),
                answer.clone(),
                10,
                true, 1)
            .unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player_fast.clone()).unwrap();
            HuntyCore::register_player(env.clone(), hunt_id, player_mid.clone()).unwrap();
            HuntyCore::register_player(env.clone(), hunt_id, player_slow.clone()).unwrap();
        });

        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player_fast.clone(), answer.clone())
                .unwrap();
        });

        env.ledger().set_timestamp(1_700_000_050);
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player_mid.clone(), answer.clone())
                .unwrap();
        });

        env.ledger().set_timestamp(1_700_000_100);
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player_slow.clone(), answer.clone())
                .unwrap();
        });

        let fast_progress = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player_fast.clone()).unwrap()
        });
        let mid_progress = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player_mid.clone()).unwrap()
        });
        let slow_progress = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player_slow.clone()).unwrap()
        });

        assert_eq!(fast_progress.total_score, 20);
        assert_eq!(mid_progress.total_score, 15);
        assert_eq!(slow_progress.total_score, 10);

        let board = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_hunt_leaderboard(env.clone(), hunt_id, 3, 0).unwrap().entries
        });

        assert_eq!(board.len(), 3);
        assert_eq!(board.get(0).unwrap().player, player_fast);
        assert_eq!(board.get(0).unwrap().score, 20);
        assert_eq!(board.get(1).unwrap().player, player_mid);
        assert_eq!(board.get(1).unwrap().score, 15);
        assert_eq!(board.get(2).unwrap().player, player_slow);
        assert_eq!(board.get(2).unwrap().score, 10);
    }

    #[test]
    fn test_create_hunt_with_end_time() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Timed Hunt");
        let description = String::from_str(&env, "A hunt with an end time");
        let end_time = 1_700_086_400u64; // 1 day in the future

        let hunt = with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title.clone(),
                description.clone(),
                None,
                Some(end_time),
                0,
                None,
            )
            .unwrap();
            Storage::get_hunt(env, hunt_id).unwrap()
        });
        assert_eq!(hunt.end_time, end_time);
    }

    #[test]
    fn test_create_hunt_invalid_end_time() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Expired Hunt");
        let description = String::from_str(&env, "A hunt with an expired end time");
        let end_time = 1_700_000_000; // equal to current time (invalid)

        let result = with_core_contract(&env, |env, _cid| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title.clone(),
                description.clone(),
                None,
                Some(end_time),
            )
        });
        assert_eq!(result, Err(HuntErrorCode::InvalidEndTime));

        let end_time_past = 1_699_999_999; // in the past (invalid)
        let result_past = with_core_contract(&env, |env, _cid| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title.clone(),
                description.clone(),
                None,
                Some(end_time_past),
            )
        });
        assert_eq!(result_past, Err(HuntErrorCode::InvalidEndTime));
    }


    #[test]
    fn test_create_hunt_empty_title() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "");
        let description = String::from_str(&env, "Valid description");

        let result = with_core_contract(&env, |env, _cid| {
            HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
        });

        assert_eq!(result, Err(HuntErrorCode::InvalidTitle));
    }

    #[test]
    fn test_create_hunt_title_too_long() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        // Create a title longer than 200 characters
        let long_title = String::from_str(&env, &"a".repeat(201));
        let description = String::from_str(&env, "Valid description");

        let result = with_core_contract(&env, |env, _cid| {
            HuntyCore::create_hunt(env.clone(), creator, long_title, description, None, None, 0, None)
        });

        assert_eq!(result, Err(HuntErrorCode::InvalidTitle));
    }

    #[test]
    fn test_create_hunt_title_exactly_max_length() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        // Create a title exactly 200 characters (should be valid)
        let title = String::from_str(&env, &"a".repeat(200));
        let description = String::from_str(&env, "Valid description");

        let result = with_core_contract(&env, |env, _cid| {
            HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
        });

        assert!(result.is_ok());
    }

    #[test]
    fn test_add_clues_success() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);

        let (ids, hunt, clues) = with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Batch Hunt"),
                String::from_str(env, "Description"),
                None,
                None,
            )
            .unwrap();
            let clues = Vec::from_array(
                env,
                [
                    BatchClueInput {
                        question: String::from_str(env, "Q1"),
                        answer: String::from_str(env, "a1"),
                        points: 10,
                        is_required: true,
                        difficulty: 1,
                    },
                    BatchClueInput {
                        question: String::from_str(env, "Q2"),
                        answer: String::from_str(env, "a2"),
                        points: 20,
                        is_required: false,
                        difficulty: 3,
                    },
                ],
            );

            let ids = HuntyCore::add_clues(env.clone(), hunt_id, clues).unwrap();
            let hunt = Storage::get_hunt(env, hunt_id).unwrap();
            let stored = HuntyCore::list_clues(env.clone(), hunt_id, 0, 10);
            (ids, hunt, stored)
        });

        assert_eq!(ids.len(), 2);
        assert_eq!(ids.get(0).unwrap(), 1);
        assert_eq!(ids.get(1).unwrap(), 2);
        assert_eq!(hunt.total_clues, 2);
        assert_eq!(hunt.required_clues, 1);
        assert_eq!(clues.len(), 2);
        assert_eq!(clues.get(0).unwrap().points, 10);
        assert_eq!(clues.get(1).unwrap().difficulty, 3);
    }

    #[test]
    fn test_add_clues_rejects_batch_over_clue_limit() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);

        let clue_count = with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Batch Hunt"),
                String::from_str(env, "Description"),
                None,
                None,
            )
            .unwrap();

            for _ in 0..99 {
                HuntyCore::add_clue(
                    env.clone(),
                    hunt_id,
                    String::from_str(env, "Q"),
                    String::from_str(env, "a"),
                    1,
                    false,
                    1,
                )
                .unwrap();
            }

            let clues = Vec::from_array(
                env,
                [
                    BatchClueInput {
                        question: String::from_str(env, "Q100"),
                        answer: String::from_str(env, "a100"),
                        points: 1,
                        is_required: false,
                        difficulty: 1,
                    },
                    BatchClueInput {
                        question: String::from_str(env, "Q101"),
                        answer: String::from_str(env, "a101"),
                        points: 1,
                        is_required: false,
                        difficulty: 1,
                    },
                ],
            );

            let err = HuntyCore::add_clues(env.clone(), hunt_id, clues).unwrap_err();
            assert_eq!(err, HuntErrorCode::TooManyClues);
            Storage::get_clue_counter(env, hunt_id)
        });

        assert_eq!(clue_count, 99);
    }

    #[test]
    fn test_add_clues_invalid_hunt_status_not_draft() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Batch Hunt"),
                String::from_str(env, "Description"),
                None,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(
                env.clone(),
                hunt_id,
                String::from_str(env, "Required"),
                String::from_str(env, "a"),
                1,
                true,
                1,
            )
            .unwrap();
            let mut hunt = Storage::get_hunt(env, hunt_id).unwrap();
            hunt.reward_config =
                crate::types::HuntRewardConfig::new(env, 100, false, None, 1, 0, 0);
            Storage::save_hunt(env, &hunt);
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            let clues = Vec::from_array(
                env,
                [BatchClueInput {
                    question: String::from_str(env, "Q2"),
                    answer: String::from_str(env, "a2"),
                    points: 1,
                    is_required: false,
                    difficulty: 1,
                }],
            );

            let err = HuntyCore::add_clues(env.clone(), hunt_id, clues).unwrap_err();
            assert_eq!(err, HuntErrorCode::InvalidHuntStatus);
        });
    }

    #[test]
    fn test_create_hunt_description_too_long() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Valid Title");
        // Create a description longer than 2000 characters
        let long_description = String::from_str(&env, &"a".repeat(2001));

        let result = with_core_contract(&env, |env, _cid| {
            HuntyCore::create_hunt(env.clone(), creator, title, long_description, None, None, 0, None)
        });

        assert_eq!(result, Err(HuntErrorCode::InvalidDescription));
    }

    #[test]
    fn test_create_hunt_description_exactly_max_length() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Valid Title");
        // Create a description exactly 2000 characters (should be valid)
        let description = String::from_str(&env, &"a".repeat(2000));

        let result = with_core_contract(&env, |env, _cid| {
            HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
        });

        assert!(result.is_ok());
    }

    #[test]
    fn test_create_hunt_unique_ids() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let title1 = String::from_str(&env, "Hunt 1");
        let title2 = String::from_str(&env, "Hunt 2");
        let title3 = String::from_str(&env, "Hunt 3");
        let description = String::from_str(&env, "Description");

        let (hunt_id1, hunt_id2, hunt_id3) = with_core_contract(&env, |env, _cid| {
            let hunt_id1 = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title1,
                description.clone(),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            let hunt_id2 = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title2,
                description.clone(),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            let hunt_id3 = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title3,
                description,
                None,
                None,
                0,
                None,
            )
            .unwrap();
            (hunt_id1, hunt_id2, hunt_id3)
        });

        // Verify IDs are unique and sequential
        assert_eq!(hunt_id1, 1);
        assert_eq!(hunt_id2, 2);
        assert_eq!(hunt_id3, 3);
        assert_ne!(hunt_id1, hunt_id2);
        assert_ne!(hunt_id2, hunt_id3);
    }

    #[test]
    fn test_create_hunt_twice_returns_different_ids() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Test Hunt");
        let description = String::from_str(&env, "Description");

        let (first_hunt_id, second_hunt_id) = with_core_contract(&env, |env, _cid| {
            let first_hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title.clone(),
                description.clone(),
                None,
                None,
            )
            .unwrap();
            let second_hunt_id =
                HuntyCore::create_hunt(env.clone(), creator, title, description, None, None)
                    .unwrap();

            (first_hunt_id, second_hunt_id)
        });

        assert_ne!(first_hunt_id, second_hunt_id);
    }

    #[test]
    fn test_create_hunt_different_creators() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator1 = Address::generate(&env);
        let creator2 = Address::generate(&env);
        let title = String::from_str(&env, "Test Hunt");
        let description = String::from_str(&env, "Description");

        let (hunt_id1, hunt_id2, hunt1, hunt2) = with_core_contract(&env, |env, _cid| {
            let hunt_id1 = HuntyCore::create_hunt(
                env.clone(),
                creator1.clone(),
                title.clone(),
                description.clone(),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            let hunt_id2 = HuntyCore::create_hunt(
                env.clone(),
                creator2.clone(),
                title,
                description,
                None,
                None,
                0,
                None,
            )
            .unwrap();
            let hunt1 = Storage::get_hunt(env.clone(), hunt_id1).unwrap();
            let hunt2 = Storage::get_hunt(env.clone(), hunt_id2).unwrap();
            (hunt_id1, hunt_id2, hunt1, hunt2)
        });

        assert_eq!(hunt1.creator, creator1);
        assert_eq!(hunt2.creator, creator2);
        assert_ne!(hunt1.creator, hunt2.creator);
    }

    #[test]
    fn test_create_hunt_counter_increments() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Test Hunt");
        let description = String::from_str(&env, "Description");

        let (start_counter, hunt_id1, counter_after_1, hunt_id2, counter_after_2, hunt_count) =
            with_core_contract(&env, |env, _cid| {
                // Verify counter starts at 0
                let start_counter = Storage::get_hunt_counter(env);

                // Create first hunt
                let hunt_id1 = HuntyCore::create_hunt(
                    env.clone(),
                    creator.clone(),
                    title.clone(),
                    description.clone(),
                    None,
                    None,
                    0,
                    None,
                )
                .unwrap();

                // Counter should be 1 after first hunt
                let counter_after_1 = Storage::get_hunt_counter(env);

                // Create second hunt
                let hunt_id2 = HuntyCore::create_hunt(
                    env.clone(),
                    creator.clone(),
                    title,
                    description,
                    None,
                    None,
                    0,
                    None,
                )
                .unwrap();

                // Counter should be 2 after second hunt
                let counter_after_2 = Storage::get_hunt_counter(env);
                let hunt_count = HuntyCore::get_hunt_count(env.clone());

                (
                    start_counter,
                    hunt_id1,
                    counter_after_1,
                    hunt_id2,
                    counter_after_2,
                    hunt_count,
                )
            });

        assert_eq!(start_counter, 0);
        assert_eq!(counter_after_1, 1);
        assert_eq!(hunt_id1, 1);
        assert_eq!(counter_after_2, 2);
        assert_eq!(hunt_id2, 2);
        assert_eq!(hunt_count, 2);
    }

    #[test]
    fn test_create_hunt_default_reward_config() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Test Hunt");
        let description = String::from_str(&env, "Description");

        let hunt = with_core_contract(&env, |env, _cid| {
            let hunt_id =
                HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
                    .unwrap();
            Storage::get_hunt(env, hunt_id).unwrap()
        });
        let reward_config = hunt.reward_config;

        // Verify default reward config values
        assert_eq!(reward_config.xlm_pool, 0);
        assert_eq!(reward_config.nft_enabled, false);
        assert_eq!(reward_config.distribution_config.nft_contract, None);
        assert_eq!(reward_config.max_winners, 0);
        assert_eq!(reward_config.claimed_count, 0);
    }

    #[test]
    fn test_create_hunt_created_at_timestamp() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Test Hunt");
        let description = String::from_str(&env, "Description");

        let (hunt, current_time) = with_core_contract(&env, |env, _cid| {
            let hunt_id =
                HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
                    .unwrap();
            (
                Storage::get_hunt(env, hunt_id).unwrap(),
                env.ledger().timestamp(),
            )
        });

        // Created timestamp should be set and reasonable (within a few seconds)
        assert!(hunt.created_at > 0);
        assert!(hunt.created_at <= current_time);
        // Allow some small time difference for test execution
        assert!(current_time - hunt.created_at < 10);
    }

    #[test]
    fn test_create_hunt_from_template_copies_completed_hunt_clues() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let contract_id = env.register_contract(None, super::HuntyCore);

        let template_creator = Address::generate(&env);
        let new_creator = Address::generate(&env);
        let player = Address::generate(&env);
        let title = String::from_str(&env, "Template Hunt");
        let description = String::from_str(&env, "Completed hunt used as a template");
        let cloned_title = String::from_str(&env, "Remixed Hunt");
        let cloned_description = String::from_str(&env, "Fresh draft from template");
        let q1 = String::from_str(&env, "What is 2 + 2?");
        let q2 = String::from_str(&env, "What is 3 + 3?");
        let a1 = String::from_str(&env, "four");
        let a2 = String::from_str(&env, "six");

        let template_hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                template_creator.clone(),
                title,
                description,
                None,
                None,
            )
            .unwrap()
        });

        let mut template_hunt = as_core_contract(&env, &contract_id, |env| {
            Storage::get_hunt(env, template_hunt_id).unwrap()
        });
        template_hunt.reward_config = crate::types::HuntRewardConfig::new(&env, 0, false, None, 1, 0, 0);
        as_core_contract(&env, &contract_id, |env| {
            Storage::save_hunt(env, &template_hunt);
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(env.clone(), template_hunt_id, q1, a1.clone(), 10, true, 1).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(env.clone(), template_hunt_id, q2, a2.clone(), 20, false, 1)
                .unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::activate_hunt(env.clone(), template_hunt_id, template_creator.clone())
                .unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), template_hunt_id, player.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(
                env.clone(),
                template_hunt_id,
                1,
                player.clone(),
                a1.clone(),
            )
            .unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::complete_hunt(env.clone(), template_hunt_id, player.clone()).unwrap();
        });

        let template_hunt = as_core_contract(&env, &contract_id, |env| {
            Storage::get_hunt(env, template_hunt_id).unwrap()
        });
        let template_clues =
            as_core_contract(&env, &contract_id, |env| Storage::list_clues_for_hunt(env, template_hunt_id, 0, 100));

        let cloned_hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt_from_template(
                env.clone(),
                template_hunt_id,
                new_creator.clone(),
                cloned_title,
                cloned_description,
                None,
                None,
            )
            .unwrap()
        });

        let cloned_hunt =
            as_core_contract(&env, &contract_id, |env| Storage::get_hunt(env, cloned_hunt_id).unwrap());
        let cloned_clues =
            as_core_contract(&env, &contract_id, |env| Storage::list_clues_for_hunt(env, cloned_hunt_id, 0, 100));

        assert_eq!(template_hunt.status, HuntStatus::Completed);
        assert_eq!(cloned_hunt.status, HuntStatus::Draft);
        assert_eq!(cloned_hunt.creator, new_creator);
        assert_eq!(cloned_hunt.total_clues, 2);
        assert_eq!(cloned_hunt.required_clues, 1);
        assert_eq!(template_clues.len(), cloned_clues.len());

        for i in 0..template_clues.len() {
            assert_eq!(template_clues.get(i).unwrap(), cloned_clues.get(i).unwrap());
        }
    }

    #[test]
    fn test_create_hunt_from_template_rejects_incomplete_template() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let contract_id = env.register_contract(None, super::HuntyCore);

        let creator = Address::generate(&env);
        let new_creator = Address::generate(&env);
        let title = String::from_str(&env, "Template Hunt");
        let description = String::from_str(&env, "Not completed yet");
        let q = String::from_str(&env, "Question?");
        let a = String::from_str(&env, "answer");

        let template_hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title,
                description,
                None,
                None,
            )
            .unwrap()
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(env.clone(), template_hunt_id, q, a, 10, true, 1).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::activate_hunt(env.clone(), template_hunt_id, creator.clone()).unwrap();
        });

        let err = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt_from_template(
                env.clone(),
                template_hunt_id,
                new_creator,
                String::from_str(env, "Cloned"),
                String::from_str(env, "Draft from template"),
                None,
                None,
            )
            .unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::InvalidHuntStatus);
    }

    // ========== add_clue() / get_clue() / list_clues() Tests ==========

    #[test]
    fn test_add_clue_success() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Test Hunt");
        let description = String::from_str(&env, "Description");
        let question = String::from_str(&env, "What is 2 + 2?");
        let answer = String::from_str(&env, "four");

        let (hunt_id, clue_id, hunt, info) = with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title,
                description.clone(),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            let clue_id =
                HuntyCore::add_clue(env.clone(), hunt_id, question.clone(), answer, 10, true, 1)
                    .unwrap();
            let hunt = Storage::get_hunt(env, hunt_id).unwrap();
            let info = HuntyCore::get_clue(env.clone(), hunt_id, clue_id).unwrap();
            (hunt_id, clue_id, hunt, info)
        });

        assert_eq!(hunt_id, 1);
        assert_eq!(clue_id, 1);
        assert_eq!(hunt.total_clues, 1);
        assert_eq!(info.clue_id, 1);
        assert_eq!(info.question, question);
        assert_eq!(info.points, 10);
        assert!(info.is_required);
    }

    #[test]
    #[should_panic]
    fn test_add_clue_unauthorized() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        // Do NOT mock auth â€” require_auth(creator) will fail.
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Test Hunt");
        let description = String::from_str(&env, "Description");
        let question = String::from_str(&env, "What is 2 + 2?");
        let answer = String::from_str(&env, "four");

        with_core_contract(&env, |env, _cid| {
            let hunt_id =
                HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
                    .unwrap();
            let _ = HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 10, true, 1);
        });
    }

    #[test]
    fn test_add_clue_sequential_ids() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let q1 = String::from_str(&env, "Q1");
        let q2 = String::from_str(&env, "Q2");
        let q3 = String::from_str(&env, "Q3");
        let a = String::from_str(&env, "a");

        let (id1, id2, id3) = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
                .unwrap();
            let id1 = HuntyCore::add_clue(env.clone(), hid, q1, a.clone(), 1, false, 1).unwrap();
            let id2 = HuntyCore::add_clue(env.clone(), hid, q2, a.clone(), 1, false, 1).unwrap();
            let id3 = HuntyCore::add_clue(env.clone(), hid, q3, a, 1, false, 1).unwrap();
            (id1, id2, id3)
        });

        assert_eq!(id1, 1);
        assert_eq!(id2, 2);
        assert_eq!(id3, 3);
    }

    #[test]
    fn test_add_clue_answer_normalization_and_hashing() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let question = String::from_str(&env, "Same answer?");
        let answer1 = String::from_str(&env, "  ANSWER  ");
        let answer2 = String::from_str(&env, "answer");

        let (hash1, hash2) = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(
                env.clone(),
                creator,
                title,
                description.clone(),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            let cid =
                HuntyCore::add_clue(env.clone(), hid, question.clone(), answer1, 5, false, 1).unwrap();
            let c = Storage::get_clue(env, hid, cid).unwrap();
            let h1 = c.answer_hashes.get(0).unwrap();
            let hid2 = HuntyCore::create_hunt(
                env.clone(),
                Address::generate(&env),
                String::from_str(&env, "H2"),
                description,
                None,
                None,
                0,
                None,
            )
            .unwrap();
            let _cid2 =
                HuntyCore::add_clue(env.clone(), hid2, question, answer2, 5, false, 1).unwrap();
            let c2 = Storage::get_clue(env, hid2, _cid2).unwrap();
            let h2 = c2.answer_hashes.get(0).unwrap();
            (h1, h2)
        });

        assert_eq!(
            hash1, hash2,
            "normalized '  ANSWER  ' and 'answer' must hash the same"
        );
    }

    #[test]
    fn test_add_clue_whitespace_answer_normalization_and_hashing() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let question = String::from_str(&env, "Whitespace answer?");
        let answer1 = String::from_str(&env, "\t\n answer \r\n");
        let answer2 = String::from_str(&env, "answer");

        let (hash1, hash2) = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(
                env.clone(),
                creator,
                title,
                description.clone(),
                None,
                None,
            )
            .unwrap();
            let cid = HuntyCore::add_clue(env.clone(), hid, question.clone(), answer1, 5, false, 1)
                .unwrap();
            let c = Storage::get_clue(env, hid, cid).unwrap();
            let h1 = c.answer_hashes.get(0).unwrap();
            let hid2 = HuntyCore::create_hunt(
                env.clone(),
                Address::generate(&env),
                String::from_str(&env, "H2"),
                description,
                None,
                None,
            )
            .unwrap();
            let _cid2 = HuntyCore::add_clue(env.clone(), hid2, question, answer2, 5, false, 1).unwrap();
            let c2 = Storage::get_clue(env, hid2, _cid2).unwrap();
            let h2 = c2.answer_hashes.get(0).unwrap();
            (h1, h2)
        });

        assert_eq!(
            hash1, hash2,
            "normalized '\t\n answer \r\n' and 'answer' must hash the same"
        );
    }

    #[test]
    fn test_add_clue_unicode_answer_normalization_and_hashing() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let question = String::from_str(&env, "Same answer?");
        let answer1 = String::from_str(&env, "CafÃ©");
        let answer2 = String::from_str(&env, "cafÃ©");

        let (hash1, hash2) = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(
                env.clone(),
                creator,
                title,
                description.clone(),
                None,
                None,
            )
            .unwrap();
            let cid =
                HuntyCore::add_clue(env.clone(), hid, question.clone(), answer1, 5, false, 1).unwrap();
            let c = Storage::get_clue(env, hid, cid).unwrap();
            let h1 = c.answer_hashes.get(0).unwrap();
            let hid2 = HuntyCore::create_hunt(
                env.clone(),
                Address::generate(&env),
                String::from_str(&env, "H2"),
                description,
                None,
                None,
            )
            .unwrap();
            let _cid2 =
                HuntyCore::add_clue(env.clone(), hid2, question, answer2, 5, false, 1).unwrap();
            let c2 = Storage::get_clue(env, hid2, _cid2).unwrap();
            let h2 = c2.answer_hashes.get(0).unwrap();
            (h1, h2)
        });

        assert_eq!(
            hash1, hash2,
            "normalized 'CafÃ©' and 'cafÃ©' must hash the same"
        );
    }

    #[test]
    fn test_get_clue_excludes_answer_hash() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let question = String::from_str(&env, "Secret?");
        let answer = String::from_str(&env, "secret");

        let info = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
                .unwrap();
            let _ = HuntyCore::add_clue(env.clone(), hid, question.clone(), answer, 7, true, 1);
            HuntyCore::get_clue(env.clone(), hid, 1).unwrap()
        });

        // Prove at compile-time that `ClueInfo` has exactly these fields, and NO `answer_hash` field.
        // The raw `Clue` (with hash) cannot be fetched through the public API (`get_clue` returns `ClueInfo`).
        let ClueInfo {
            clue_id,
            question: ret_question,
            points,
            is_required,
            ..
        } = info;

        assert_eq!(clue_id, 1);
        assert_eq!(ret_question, question);
        assert_eq!(points, 7);
        assert!(is_required);
    }

    #[test]
    fn test_get_clue_not_found() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");

        let err = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
                .unwrap();
            HuntyCore::get_clue(env.clone(), hid, 999).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::ClueNotFound);
    }

    #[test]
    fn test_list_clues_empty() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");

        let list = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator.clone(), title.clone(), description.clone(), None, None)
                .unwrap();
            HuntyCore::list_clues(env.clone(), hid, 0, 10)
        });

        let expected = Vec::new(&env);
        assert_eq!(list, expected);
    }

    #[test]
    fn test_list_clues_returns_all() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let q1 = String::from_str(&env, "Q1");
        let q2 = String::from_str(&env, "Q2");
        let a = String::from_str(&env, "a");

        let list = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
                .unwrap();
            HuntyCore::add_clue(env.clone(), hid, q1, a.clone(), 1, false, 1).unwrap();
            HuntyCore::add_clue(env.clone(), hid, q2, a, 2, true, 1).unwrap();
            HuntyCore::list_clues(env.clone(), hid, 0, 10)
        });

        assert_eq!(list.len(), 2);
        let c1 = list.get(0).unwrap();
        let c2 = list.get(1).unwrap();
        assert_eq!(c1.clue_id, 1);
        assert_eq!(c2.clue_id, 2);
        assert_eq!(c1.points, 1);
        assert_eq!(c2.points, 2);
        assert!(!c1.is_required);
        assert!(c2.is_required);
    }

    #[test]
    fn test_list_clues_pagination() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let q1 = String::from_str(&env, "Q1");
        let q2 = String::from_str(&env, "Q2");
        let q3 = String::from_str(&env, "Q3");
        let a = String::from_str(&env, "a");

        let (list1, list2, list_all) = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None)
                .unwrap();
            HuntyCore::add_clue(env.clone(), hid, q1, a.clone(), 1, false, 1).unwrap();
            HuntyCore::add_clue(env.clone(), hid, q2, a.clone(), 2, true, 1).unwrap();
            HuntyCore::add_clue(env.clone(), hid, q3, a, 3, false, 1).unwrap();
            (
                HuntyCore::list_clues(env.clone(), hid, 0, 2),
                HuntyCore::list_clues(env.clone(), hid, 2, 2),
                HuntyCore::list_clues(env.clone(), hid, 0, 10),
            )
        });

        // Validate results
        assert_eq!(list1.len(), 2);
        assert_eq!(list2.len(), 1);
        assert_eq!(list_all.len(), 3);
        
        assert_eq!(list1.get(0).unwrap().clue_id, 1);
        assert_eq!(list1.get(1).unwrap().clue_id, 2);
        assert_eq!(list2.get(0).unwrap().clue_id, 3);
    }

    #[test]
    fn test_add_clue_hunt_not_found() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let err = with_core_contract(&env, |env, _cid| {
            HuntyCore::add_clue(env.clone(), 9999, question, answer, 1, false, 1).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::HuntNotFound);
    }

    #[test]
    fn test_add_clue_invalid_question_empty() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let empty = String::from_str(&env, "");
        let answer = String::from_str(&env, "a");

        let err = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
                .unwrap();
            HuntyCore::add_clue(env.clone(), hid, empty, answer, 1, false, 1).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::InvalidQuestion);
    }

    #[test]
    fn test_add_clue_invalid_answer_empty() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let question = String::from_str(&env, "Q");
        let empty = String::from_str(&env, "");

        let err = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
                .unwrap();
            HuntyCore::add_clue(env.clone(), hid, question, empty, 1, false, 1).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::InvalidAnswer);
    }

    #[test]
    fn test_add_clue_invalid_answer_whitespace_only() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let question = String::from_str(&env, "Q");
        let ws = String::from_str(&env, "   \t  ");

        let err = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
                .unwrap();
            HuntyCore::add_clue(env.clone(), hid, question, ws, 1, false, 1).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::InvalidAnswer);
    }

    #[test]
    fn test_add_clue_too_many_clues() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        const MAX_CLUES: u32 = 100;
        let err = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
                .unwrap();
            for _ in 0..MAX_CLUES {
                HuntyCore::add_clue(env.clone(), hid, question.clone(), answer.clone(), 1, false, 1)
                    .unwrap();
            }
            HuntyCore::add_clue(env.clone(), hid, question, answer, 1, false, 1).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::TooManyClues);
    }

    #[test]
    fn test_add_clue_invalid_hunt_status_not_draft() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let err = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title,
                description,
                None,
                None,
                0,
                None,
            )
            .unwrap();
            let mut h = Storage::get_hunt(env, hid).unwrap();
            h.status = HuntStatus::Active;
            Storage::save_hunt(env, &h);
            HuntyCore::add_clue(env.clone(), hid, question, answer, 1, false, 1).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::InvalidHuntStatus);
    }

    #[test]
    fn test_add_clue_after_activation_fails() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let err = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title,
                description,
                None,
                None,
            )
            .unwrap();

            // Add a required clue to allow activation
            HuntyCore::add_clue(env.clone(), hid, question.clone(), answer.clone(), 1, true, 1)
                .unwrap();

            // Activate the hunt
            HuntyCore::activate_hunt(env.clone(), hid, creator.clone()).unwrap();

            // Attempt to add a clue after activation (should fail)
            HuntyCore::add_clue(env.clone(), hid, question, answer, 1, false, 1).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::InvalidHuntStatus);
    }

    #[test]
    fn test_add_clue_invalid_question_too_long() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let long_q = String::from_str(&env, &"a".repeat(2001));
        let answer = String::from_str(&env, "a");

        let err = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None, 0, None)
                .unwrap();
            HuntyCore::add_clue(env.clone(), hid, long_q, answer, 1, false, 1).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::InvalidQuestion);
    }

    // ========== add_clue_aliases() Tests ==========

    #[test]
    fn test_add_clue_aliases_success() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let contract_id = env.register_contract(None, super::HuntyCore);

        let hid = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
            )
            .unwrap()
        });
        env.mock_all_auths();
        let cid = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(
                env.clone(),
                hid,
                String::from_str(env, "Capital of USA?"),
                String::from_str(env, "Washington"),
                10,
                true, 1)
            .unwrap()
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            let aliases = Vec::from_array(
                env,
                [
                    String::from_str(env, "Washington D.C."),
                    String::from_str(env, "DC"),
                ],
            );
            HuntyCore::add_clue_aliases(env.clone(), hid, cid, aliases).unwrap();
            let clue = Storage::get_clue(env, hid, cid).unwrap();
            assert_eq!(clue.answer_hashes.len(), 3);
        });
    }

    #[test]
    fn test_add_clue_aliases_answers_accepted() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let contract_id = env.register_contract(None, super::HuntyCore);

        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Geo Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
            )
            .unwrap()
        });
        env.mock_all_auths();
        let cid = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(
                env.clone(),
                hunt_id,
                String::from_str(env, "Capital of USA?"),
                String::from_str(env, "Washington"),
                10,
                true, 1)
            .unwrap()
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            let aliases = Vec::from_array(
                env,
                [
                    String::from_str(env, "Washington D.C."),
                    String::from_str(env, "DC"),
                ],
            );
            HuntyCore::add_clue_aliases(env.clone(), hunt_id, cid, aliases).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(
                env.clone(),
                hunt_id,
                1,
                player.clone(),
                String::from_str(env, "Washington"),
            )
            .unwrap();
        });
        let progress = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player.clone()).unwrap()
        });
        assert!(progress.is_completed);

        // Now test alias answers work â€” register a new player for each alias
        for alias in ["Washington D.C.", "DC"] {
            let p = Address::generate(&env);
            env.mock_all_auths();
            as_core_contract(&env, &contract_id, |env| {
                HuntyCore::register_player(env.clone(), hunt_id, p.clone()).unwrap();
            });
            env.mock_all_auths();
            as_core_contract(&env, &contract_id, |env| {
                HuntyCore::submit_answer(
                    env.clone(),
                    hunt_id,
                    1,
                    p.clone(),
                    String::from_str(env, alias),
                )
                .unwrap();
            });
            let progress = as_core_contract(&env, &contract_id, |env| {
                HuntyCore::get_player_progress(env.clone(), hunt_id, p.clone()).unwrap()
            });
            assert!(progress.is_completed);
        }
    }

    #[test]
    fn test_add_clue_aliases_hunt_not_found() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let aliases = Vec::from_array(&env, [String::from_str(&env, "alias")]);

        let err = with_core_contract(&env, |env, _cid| {
            HuntyCore::add_clue_aliases(env.clone(), 9999, 1, aliases).unwrap_err()
        });
        assert_eq!(err, HuntErrorCode::HuntNotFound);
    }

    #[test]
    fn test_add_clue_aliases_clue_not_found() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);

        let err = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(
                env.clone(),
                creator,
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
            )
            .unwrap();
            let aliases = Vec::from_array(env, [String::from_str(env, "alias")]);
            HuntyCore::add_clue_aliases(env.clone(), hid, 999, aliases).unwrap_err()
        });
        assert_eq!(err, HuntErrorCode::ClueNotFound);
    }

    #[test]
    fn test_add_clue_aliases_invalid_hunt_status() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let contract_id = env.register_contract(None, super::HuntyCore);

        let hid = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
            )
            .unwrap()
        });
        env.mock_all_auths();
        let cid = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(
                env.clone(),
                hid,
                String::from_str(env, "Q"),
                String::from_str(env, "a"),
                1,
                true, 1)
            .unwrap()
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            let mut h = Storage::get_hunt(env, hid).unwrap();
            h.status = HuntStatus::Active;
            Storage::save_hunt(env, &h);
        });
        env.mock_all_auths();
        let err = as_core_contract(&env, &contract_id, |env| {
            let aliases = Vec::from_array(env, [String::from_str(env, "alias")]);
            HuntyCore::add_clue_aliases(env.clone(), hid, cid, aliases).unwrap_err()
        });
        assert_eq!(err, HuntErrorCode::InvalidHuntStatus);
    }

    #[test]
    fn test_add_clue_aliases_preserves_existing_hashes() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let contract_id = env.register_contract(None, super::HuntyCore);

        let hid = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
            )
            .unwrap()
        });
        env.mock_all_auths();
        let cid = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(
                env.clone(),
                hid,
                String::from_str(env, "Q"),
                String::from_str(env, "original"),
                5,
                true, 1)
            .unwrap()
        });
        let original_hash = as_core_contract(&env, &contract_id, |env| {
            let clue_before = Storage::get_clue(env, hid, cid).unwrap();
            clue_before.answer_hashes.get(0).unwrap()
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            let aliases = Vec::from_array(
                env,
                [String::from_str(env, "alias1"), String::from_str(env, "alias2")],
            );
            HuntyCore::add_clue_aliases(env.clone(), hid, cid, aliases).unwrap();
            let clue_after = Storage::get_clue(env, hid, cid).unwrap();
            assert_eq!(clue_after.answer_hashes.len(), 3);
            assert_eq!(clue_after.answer_hashes.get(0).unwrap(), original_hash);
        });
    }

    #[test]
    fn test_add_clue_aliases_empty_answer_fails() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let contract_id = env.register_contract(None, super::HuntyCore);

        let hid = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
            )
            .unwrap()
        });
        env.mock_all_auths();
        let cid = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(
                env.clone(),
                hid,
                String::from_str(env, "Q"),
                String::from_str(env, "a"),
                1,
                true, 1)
            .unwrap()
        });
        env.mock_all_auths();
        let err = as_core_contract(&env, &contract_id, |env| {
            let aliases =
                Vec::from_array(env, [String::from_str(env, ""), String::from_str(env, "valid")]);
            HuntyCore::add_clue_aliases(env.clone(), hid, cid, aliases).unwrap_err()
        });
        assert_eq!(err, HuntErrorCode::InvalidAnswer);
    }

    #[test]
    #[should_panic]
    fn test_add_clue_aliases_creator_only() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        // Do NOT mock auth â€” require_auth(attacker) will panic
        let creator = Address::generate(&env);
        let attacker = Address::generate(&env);
        let aliases = Vec::from_array(&env, [String::from_str(&env, "alias")]);

        with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
            )
            .unwrap();
            let cid = HuntyCore::add_clue(
                env.clone(),
                hid,
                String::from_str(env, "Q"),
                String::from_str(env, "a"),
                1,
                true, 1)
            .unwrap();
            let _ = HuntyCore::add_clue_aliases(env.clone(), hid, cid, aliases);
        });
    }

    #[test]
    fn test_add_clue_zero_points() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let err = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None)
                .unwrap();
            HuntyCore::add_clue(env.clone(), hid, question, answer, 0, false, 1).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::InvalidPoints);
    }

    #[test]
    fn test_add_clue_invalid_difficulty_zero() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let err = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None)
                .unwrap();
            HuntyCore::add_clue(env.clone(), hid, question, answer, 1, false, 0).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::InvalidDifficulty);
    }

    #[test]
    fn test_add_clue_invalid_difficulty_exceeds_max() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Hunt");
        let description = String::from_str(&env, "Desc");
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let err = with_core_contract(&env, |env, _cid| {
            let hid = HuntyCore::create_hunt(env.clone(), creator, title, description, None, None)
                .unwrap();
            HuntyCore::add_clue(env.clone(), hid, question, answer, 1, false, 11).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::InvalidDifficulty);
    }

    #[test]
    fn test_clue_difficulty_multiplier_in_scoring() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Question");
        let answer = String::from_str(&env, "answer");

        with_core_contract(&env, |env, _cid| {
            // Create hunt
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Test Hunt"),
                String::from_str(env, "Test description"),
                None,
                None,
            )
            .unwrap();

            // Add clue with 10 points and difficulty 3 (should give 30 points when solved)
            HuntyCore::add_clue(
                env.clone(),
                hunt_id,
                question.clone(),
                answer.clone(),
                10,
                true,
                3,
            )
            .unwrap();

            // Activate hunt
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            // Register player
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();

            // Verify initial score is 0
            let progress =
                HuntyCore::get_player_progress(env.clone(), hunt_id, player.clone()).unwrap();
            assert_eq!(progress.total_score, 0);

            // Submit correct answer
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player.clone(), answer).unwrap();

            // Verify score is 30 (10 * 3)
            let progress =
                HuntyCore::get_player_progress(env.clone(), hunt_id, player.clone()).unwrap();
            assert_eq!(progress.total_score, 30);
            assert!(progress.is_completed);
        });
    }

    #[test]
    fn test_clue_list_includes_difficulty() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator,
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
            )
            .unwrap();

            // Add clue with difficulty 5
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 20, true, 5).unwrap();

            // Get clue and verify difficulty is included
            let info = HuntyCore::get_clue(env.clone(), hunt_id, 1).unwrap();
            assert_eq!(info.difficulty, 5);
            assert_eq!(info.points, 20);

            // List clues and verify difficulty is included
            let list = HuntyCore::list_clues(env.clone(), hunt_id, 0, 10);
            assert_eq!(list.len(), 1);
            let c = list.get(0).unwrap();
            assert_eq!(c.difficulty, 5);
            assert_eq!(c.points, 20);
        });
    }

    #[test]
    fn test_activate_hunt_success() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let title = String::from_str(&env, "Test Hunt");
        let description = String::from_str(&env, "This is a test hunt description");

        let question = String::from_str(&env, "Valid question");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title,
                description,
                None,
                None,
                0,
                None,
            )
            .unwrap();

            // Add a VALID required clue
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();

            // Activate hunt
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            let hunt = Storage::get_hunt(env, hunt_id).unwrap();
            assert_eq!(hunt.status, HuntStatus::Active);
            assert!(hunt.activated_at > 0);
        });
    }

    #[test]
    fn test_activate_hunt_not_found() {
        let env = Env::default();
        let creator = Address::generate(&env);

        with_core_contract(&env, |env, _cid| {
            let err = HuntyCore::activate_hunt(env.clone(), 999, creator.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::HuntNotFound);
        });
    }

    #[test]
    fn test_activate_hunt_unauthorized() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let attacker = Address::generate(&env);

        let title = String::from_str(&env, "Test Hunt");
        let description = String::from_str(&env, "Test description");

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title,
                description,
                None,
                None,
                0,
                None,
            )
            .unwrap();

            let err = HuntyCore::activate_hunt(env.clone(), hunt_id, attacker.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::Unauthorized);
        });
    }

    #[test]
    fn test_activate_hunt_no_clues() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);

        let title = String::from_str(&env, "Test Hunt");
        let description = String::from_str(&env, "Test description");

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title,
                description,
                None,
                None,
                0,
                None,
            )
            .unwrap();

            let err = HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::NoCluesAdded);
        });
    }

    #[test]
    fn test_activate_hunt_no_required_clues() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);

        let title = String::from_str(&env, "Test Hunt");
        let description = String::from_str(&env, "Test description");
        let question = String::from_str(&env, "Optional clue question");
        let answer = String::from_str(&env, "answer");

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                title,
                description,
                None,
                None,
                0,
                None,
            )
            .unwrap();

            // Add only an optional clue (is_required = false)
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, false, 1).unwrap();

            // Activating should fail because there are no required clues
            let err = HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::NoRequiredClues);
        });
    }

    #[test]
    fn test_activate_hunt_end_time_in_past() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);

        let question = String::from_str(&env, "Valid question");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            // Create a hunt with end_time in the past
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Expired Hunt"),
                String::from_str(env, "This hunt has an end_time in the past"),
                Some(1_699_999_999), // end_time < current_time (1_700_000_000)
                None,
                0,
            )
            .unwrap();

            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true).unwrap();

            let err = HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::HuntEndTimeInPast);
        });
    }

    #[test]
    fn test_deactivate_hunt_success() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let question = String::from_str(&env, "Valid question");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            // Create hunt
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Test Hunt"),
                String::from_str(env, "Test description"),
                None,
                None,
                0,
                None,
            )
            .unwrap();

            // Add a VALID clue first
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();

            // Activate hunt
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            // Deactivate hunt â€” status must be Paused, not Draft (issue #91).
            HuntyCore::deactivate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            let hunt = Storage::get_hunt(env, hunt_id).unwrap();
            assert_eq!(hunt.status, HuntStatus::Paused);
        });
    }

    // â”€â”€ Issue #91: Paused-state tests â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€â”€

    #[test]
    fn test_deactivate_sets_paused_not_draft() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");
        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(), creator.clone(),
                String::from_str(env, "Hunt"), String::from_str(env, "Desc"), None, None,
            ).unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::deactivate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            let hunt = Storage::get_hunt(env, hunt_id).unwrap();
            assert_eq!(hunt.status, HuntStatus::Paused);
            assert_ne!(hunt.status, HuntStatus::Draft);
        });
    }

    #[test]
    fn test_reactivate_from_paused_succeeds() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");
        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(), creator.clone(),
                String::from_str(env, "Hunt"), String::from_str(env, "Desc"), None, None,
            ).unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::deactivate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            let hunt = Storage::get_hunt(env, hunt_id).unwrap();
            assert_eq!(hunt.status, HuntStatus::Active);
        });
    }

    #[test]
    fn test_deactivate_draft_hunt_fails() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");
        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(), creator.clone(),
                String::from_str(env, "Hunt"), String::from_str(env, "Desc"), None, None,
            ).unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            // Hunt is Draft â€” deactivate must reject it.
            let err = HuntyCore::deactivate_hunt(env.clone(), hunt_id, creator.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::InvalidHuntStatus);
        });
    }

    #[test]
    fn test_cannot_add_clue_to_paused_hunt() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");
        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(), creator.clone(),
                String::from_str(env, "Hunt"), String::from_str(env, "Desc"), None, None,
            ).unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question.clone(), answer.clone(), 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::deactivate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            let err = HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, false, 1).unwrap_err();
            assert_eq!(err, HuntErrorCode::InvalidHuntStatus);
        });
    }

    #[test]
    fn test_register_player_blocked_when_paused() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");
        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(), creator.clone(),
                String::from_str(env, "Hunt"), String::from_str(env, "Desc"), None, None,
            ).unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::deactivate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            let err = HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::InvalidHuntStatus);
        });
    }

    #[test]
    fn test_cancel_from_paused_succeeds() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");
        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(), creator.clone(),
                String::from_str(env, "Hunt"), String::from_str(env, "Desc"), None, None,
            ).unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::deactivate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::cancel_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            let hunt = Storage::get_hunt(env, hunt_id).unwrap();
            assert_eq!(hunt.status, HuntStatus::Cancelled);
        });
    }

    #[test]
    fn test_deactivate_hunt_not_found() {
        let env = Env::default();
        let creator = Address::generate(&env);

        with_core_contract(&env, |env, _cid| {
            let err = HuntyCore::deactivate_hunt(env.clone(), 404, creator.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::HuntNotFound);
        });
    }

    #[test]
    fn test_deactivate_hunt_unauthorized() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let attacker = Address::generate(&env);
        let question = String::from_str(&env, "Valid question");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            // Create hunt
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Test Hunt"),
                String::from_str(env, "Test description"),
                None,
                None,
                0,
                None,
            )
            .unwrap();

            // Add a VALID clue first
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();

            // Activate hunt
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            // Deactivate hunt
            let err =
                HuntyCore::deactivate_hunt(env.clone(), hunt_id, attacker.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::Unauthorized);
        });
    }

    #[test]
    fn test_cancel_hunt_from_active_success() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let question = String::from_str(&env, "Valid question");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            // Create hunt
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Test Hunt"),
                String::from_str(env, "Test description"),
                None,
                None,
                0,
                None,
            )
            .unwrap();

            // Add a VALID clue first
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();

            // Activate hunt
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            // Cancelled hunt
            HuntyCore::cancel_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            let hunt = Storage::get_hunt(env, hunt_id).unwrap();
            assert_eq!(hunt.status, HuntStatus::Cancelled);

            let status_event = find_hunt_status_changed_event(&env)
                .expect("expected HuntStatusChanged event after cancellation");
            assert_eq!(status_event.hunt_id, hunt_id);
            assert_eq!(status_event.old_status, HuntStatus::Active);
            assert_eq!(status_event.new_status, HuntStatus::Cancelled);
            assert!(status_event.changed_at > 0);
        });
    }

    #[test]
    fn test_cancel_hunt_emits_canceller_and_timestamp() {
        let env = Env::default();
        let cancelled_at = 1_700_000_123;
        env.ledger().set_timestamp(cancelled_at);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let question = String::from_str(&env, "Valid question");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Test Hunt"),
                String::from_str(env, "Test description"),
                None,
                None,
            )
            .unwrap();

            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::cancel_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            let events = env.events().all();
            let (contract, topics, data): (Address, Vec<Val>, Val) =
                events.get(events.len() - 1).unwrap();
            assert_eq!(contract, cid.clone().into());
            assert_eq!(topics.len(), 2);
            assert_eq!(
                Symbol::try_from_val(env, &topics.get(0).unwrap()).unwrap(),
                Symbol::new(env, "HuntCancelled")
            );
            assert_eq!(u64::try_from_val(env, &topics.get(1).unwrap()).unwrap(), hunt_id);

            let event = HuntCancelledEvent::try_from_val(env, &data).unwrap();
            assert_eq!(
                event,
                HuntCancelledEvent {
                    hunt_id,
                    cancelled_by: creator.clone(),
                    cancelled_at,
                }
            );
        });
    }

    #[test]
    fn test_cancel_hunt_refunds_reward_pool_balance() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let question = String::from_str(&env, "Valid question");
        let answer = String::from_str(&env, "a");

        let core_id = env.register_contract(None, super::HuntyCore);
        let (reward_manager_id, token_address, _) = setup_reward_manager(&env, None);
        let sac = token::StellarAssetClient::new(&env, &token_address);
        sac.mint(&creator, &5_000);

        let hunt_id = as_core_contract(&env, &core_id, |env| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Refund Hunt"),
                String::from_str(env, "Should refund on cancel"),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::set_reward_manager(env.clone(), creator.clone(), reward_manager_id.clone());
            hunt_id
        });

        env.mock_all_auths();
        env.as_contract(&reward_manager_id, || {
            RewardManager::create_reward_pool(env.clone(), creator.clone(), hunt_id, 0).unwrap();
        });
        env.mock_all_auths();
        env.as_contract(&reward_manager_id, || {
            RewardManager::fund_reward_pool(env.clone(), creator.clone(), hunt_id, 5_000).unwrap();
        });

        env.mock_all_auths();
        as_core_contract(&env, &core_id, |env| {
            HuntyCore::cancel_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        env.as_contract(&reward_manager_id, || {
            assert_eq!(RewardManager::get_pool_balance(env.clone(), hunt_id), 0);
        });

        let token_client = token::Client::new(&env, &token_address);
        assert_eq!(token_client.balance(&creator), 5_000);
        assert_eq!(token_client.balance(&reward_manager_id), 0);
    }

    #[test]
    fn test_cancel_hunt_not_found() {
        let env = Env::default();
        env.mock_all_auths();
        let creator = Address::generate(&env);

        with_core_contract(&env, |env, _cid| {
            let err = HuntyCore::cancel_hunt(env.clone(), 999, creator.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::HuntNotFound);
        });
    }

    #[test]
    #[should_panic]
    fn test_cancel_hunt_requires_creator_auth() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);

        let creator = Address::generate(&env);

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Test Hunt"),
                String::from_str(env, "Test description"),
                None,
                None,
            )
            .unwrap();

            HuntyCore::cancel_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });
    }

    #[test]
    fn test_cancel_hunt_unauthorized() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let attacker = Address::generate(&env);
        let question = String::from_str(&env, "Valid question");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            // Create hunt
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Test Hunt"),
                String::from_str(env, "Test description"),
                None,
                None,
                0,
                None,
            )
            .unwrap();

            // Add a VALID clue first
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();

            // Activate hunt
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            // Deactivate hunt
            let err = HuntyCore::cancel_hunt(env.clone(), hunt_id, attacker.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::Unauthorized);
        });
    }

    #[test]
    fn test_cancel_hunt_already_cancelled() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let attacker = Address::generate(&env);
        let question = String::from_str(&env, "Valid question");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            // Create hunt
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Test Hunt"),
                String::from_str(env, "Test description"),
                None,
                None,
                0,
                None,
            )
            .unwrap();

            // Add a VALID clue first
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();

            // Activate hunt
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            // Deactivate hunt
            HuntyCore::cancel_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            let err = HuntyCore::cancel_hunt(env.clone(), hunt_id, creator.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::InvalidHuntStatus);
        });
    }

    // ========== close_hunt() Tests ==========

    /// Closing an active hunt marks it Completed, distributes rewards to the
    /// completed-but-unclaimed player, and preserves that player's score.
    #[test]
    fn test_close_hunt_success_distributes_and_completes() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let player = Address::generate(&env);

        // Active hunt with one completed (unclaimed) player and no RewardManager.
        let (hunt_id, contract_id) =
            setup_completed_hunt_with_rewards(&env, &creator, &player, 5, 1000);

        // Capture score before closing to prove it is preserved.
        let score_before = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player.clone())
                .unwrap()
                .total_score
        });
        assert!(score_before > 0);

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::close_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        // Hunt is now Completed (inactive).
        let hunt = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_hunt_info(env.clone(), hunt_id).unwrap()
        });
        assert_eq!(hunt.status, HuntStatus::Completed);
        assert_eq!(hunt.reward_config.claimed_count, 1);

        // Player reward distributed but score preserved.
        let progress = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player.clone()).unwrap()
        });
        assert!(progress.reward_claimed);
        assert!(progress.is_completed);
        assert_eq!(progress.total_score, score_before);

        // HuntClosed event reports one rewarded player.
        let (_topics, closed) = as_core_contract(&env, &contract_id, |env| {
            find_event::<HuntClosedEvent>(env, "HuntClosed").expect("expected HuntClosed event")
        });
        assert_eq!(closed.hunt_id, hunt_id);
        assert_eq!(closed.rewarded_players, 1);
        assert!(closed.closed_at > 0);

        // Generic status-change event emitted Active -> Completed.
        let status_event = as_core_contract(&env, &contract_id, |env| {
            find_hunt_status_changed_event(env).expect("expected HuntStatusChanged event")
        });
        assert_eq!(status_event.old_status, HuntStatus::Active);
        assert_eq!(status_event.new_status, HuntStatus::Completed);
    }

    /// A player who has not completed the hunt keeps their progress and is not
    /// rewarded when the hunt is closed.
    #[test]
    fn test_close_hunt_preserves_incomplete_player() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let finisher = Address::generate(&env);
        let laggard = Address::generate(&env);

        // Sets up an active hunt where `finisher` has completed the single clue.
        let (hunt_id, contract_id) =
            setup_completed_hunt_with_rewards(&env, &creator, &finisher, 5, 1000);

        // A second player registers but never submits an answer.
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, laggard.clone()).unwrap();
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::close_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        // Only the finisher was rewarded.
        let hunt = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_hunt_info(env.clone(), hunt_id).unwrap()
        });
        assert_eq!(hunt.reward_config.claimed_count, 1);

        // Laggard keeps progress, unclaimed and incomplete.
        let laggard_progress = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, laggard.clone()).unwrap()
        });
        assert!(!laggard_progress.is_completed);
        assert!(!laggard_progress.reward_claimed);
    }

    /// Closing may be triggered from a Paused hunt as well as an Active one.
    #[test]
    fn test_close_hunt_from_paused_status() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let player = Address::generate(&env);

        let (hunt_id, contract_id) =
            setup_completed_hunt_with_rewards(&env, &creator, &player, 5, 1000);

        // Pause (deactivate) the hunt first.
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::deactivate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::close_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        let hunt = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_hunt_info(env.clone(), hunt_id).unwrap()
        });
        assert_eq!(hunt.status, HuntStatus::Completed);
        assert_eq!(hunt.reward_config.claimed_count, 1);
    }

    #[test]
    fn test_close_hunt_unauthorized() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let attacker = Address::generate(&env);

        let (hunt_id, contract_id) =
            setup_completed_hunt_with_rewards(&env, &creator, &player, 5, 1000);

        env.mock_all_auths();
        let err = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::close_hunt(env.clone(), hunt_id, attacker.clone()).unwrap_err()
        });
        assert_eq!(err, HuntErrorCode::Unauthorized);
    }

    #[test]
    fn test_close_hunt_not_found() {
        let env = Env::default();
        let creator = Address::generate(&env);

        with_core_contract(&env, |env, _cid| {
            let err = HuntyCore::close_hunt(env.clone(), 999, creator.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::HuntNotFound);
        });
    }

    /// A Draft hunt (never activated) cannot be closed early.
    #[test]
    fn test_close_hunt_invalid_status_draft() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let creator = Address::generate(&env);

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Draft Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap();

            let err = HuntyCore::close_hunt(env.clone(), hunt_id, creator.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::InvalidHuntStatus);
        });
    }

    /// A hunt that was already closed cannot be closed again.
    #[test]
    fn test_close_hunt_already_closed() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let player = Address::generate(&env);

        let (hunt_id, contract_id) =
            setup_completed_hunt_with_rewards(&env, &creator, &player, 5, 1000);

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::close_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        env.mock_all_auths();
        let err = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::close_hunt(env.clone(), hunt_id, creator.clone()).unwrap_err()
        });
        assert_eq!(err, HuntErrorCode::InvalidHuntStatus);
    }

    /// Closing is blocked while reward distribution is globally paused.
    #[test]
    fn test_close_hunt_rewards_paused() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let creator = Address::generate(&env);
        let player = Address::generate(&env);

        let (hunt_id, contract_id) =
            setup_completed_hunt_with_rewards(&env, &creator, &player, 5, 1000);

        as_core_contract(&env, &contract_id, |env| {
            Storage::set_pause_rewards(env, true);
        });

        env.mock_all_auths();
        let err = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::close_hunt(env.clone(), hunt_id, creator.clone()).unwrap_err()
        });
        assert_eq!(err, HuntErrorCode::RewardsPaused);

        // Hunt remains active — closing had no effect.
        let hunt = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_hunt_info(env.clone(), hunt_id).unwrap()
        });
        assert_eq!(hunt.status, HuntStatus::Active);
    }

    #[test]
    fn test_get_hunt_info() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let attacker = Address::generate(&env);
        let question = String::from_str(&env, "Valid question");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Query Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap();

            let info = HuntyCore::get_hunt_info(env.clone(), hunt_id).unwrap();

            assert_eq!(info.hunt_id, hunt_id);
            assert_eq!(info.creator, creator);
            assert_eq!(info.title, String::from_str(env, "Query Hunt"));
            assert_eq!(info.status, HuntStatus::Draft);
        });
    }

    // ========== register_player() Tests ==========

    #[test]
    fn test_register_player_success() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Valid question");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Active Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 10, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();

            let progress =
                HuntyCore::get_player_progress(env.clone(), hunt_id, player.clone()).unwrap();
            assert_eq!(progress.player, player);
            assert_eq!(progress.hunt_id, hunt_id);
            assert_eq!(progress.completed_clues.len(), 0);
            assert_eq!(progress.total_score, 0);
            assert_eq!(progress.is_completed, false);
            assert_eq!(progress.reward_claimed, false);
            assert!(progress.started_at > 0);
            assert_eq!(progress.completed_at, 0);
        });
    }

    #[test]
    fn test_max_players_limit_and_remaining_slots() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let player1 = Address::generate(&env);
        let player2 = Address::generate(&env);
        let player3 = Address::generate(&env);
        let question = String::from_str(&env, "Valid question");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Active Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 10, true, 1).unwrap();
            
            // Set max players limit to 2
            HuntyCore::set_max_players(env.clone(), hunt_id, creator.clone(), 2).unwrap();

            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            // First get remaining slots: should be 2
            let hunt = HuntyCore::get_hunt_info(env.clone(), hunt_id).unwrap();
            assert_eq!(hunt.max_players, 2);
            assert_eq!(hunt.remaining_slots, 2);

            // Register first player
            HuntyCore::register_player(env.clone(), hunt_id, player1.clone()).unwrap();
            let hunt = HuntyCore::get_hunt_info(env.clone(), hunt_id).unwrap();
            assert_eq!(hunt.remaining_slots, 1);

            // Register second player
            HuntyCore::register_player(env.clone(), hunt_id, player2.clone()).unwrap();
            let hunt = HuntyCore::get_hunt_info(env.clone(), hunt_id).unwrap();
            assert_eq!(hunt.remaining_slots, 0);

            // Attempting to register third player should fail with HuntFull
            let err = HuntyCore::register_player(env.clone(), hunt_id, player3.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::HuntFull);
        });
    }

    #[test]
    fn test_blacklist_creator_blocks_hunt_creation_and_emits_event() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let creator = Address::generate(&env);

        with_core_contract(&env, |env, cid| {
            HuntyCore::initialize_admin(env.clone(), admin.clone()).unwrap();
            HuntyCore::blacklist_creator(env.clone(), admin.clone(), creator.clone()).unwrap();

            assert!(HuntyCore::is_blacklisted(env.clone(), creator.clone()));

            let err = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Blacklisted Hunt"),
                String::from_str(env, "Should not be created"),
                None,
                None,
                5u32,
                None,
            )
            .unwrap_err();
            assert_eq!(err, HuntErrorCode::AddressBlacklisted);

            let events = env.events().all();
            let (contract, topics, data): (Address, Vec<Val>, Val) =
                events.get(events.len() - 1).unwrap();
            assert_eq!(contract, cid.clone().into());
            assert_eq!(topics.len(), 2);
            assert_eq!(
                Symbol::try_from_val(env, &topics.get(0).unwrap()).unwrap(),
                Symbol::new(env, "CreatorBlacklisted")
            );
            assert_eq!(u64::try_from_val(env, &topics.get(1).unwrap()).unwrap(), 0);

            let event = CreatorBlacklistedEvent::try_from_val(env, &data).unwrap();
            assert_eq!(event.creator, creator);
            assert_eq!(event.admin, admin);
        });
    }

    #[test]
    fn test_remove_from_blacklist_allows_hunt_creation_and_emits_event() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let creator = Address::generate(&env);

        with_core_contract(&env, |env, cid| {
            HuntyCore::initialize_admin(env.clone(), admin.clone()).unwrap();
            HuntyCore::blacklist_creator(env.clone(), admin.clone(), creator.clone()).unwrap();
            HuntyCore::remove_from_blacklist(env.clone(), admin.clone(), creator.clone())
                .unwrap();

            assert!(!HuntyCore::is_blacklisted(env.clone(), creator.clone()));

            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Recovered Hunt"),
                String::from_str(env, "Should be created"),
                None,
                None,
                5u32,
                None,
            )
            .unwrap();
            assert_eq!(hunt_id, 1);

            let events = env.events().all();
            let (_contract, topics, _data): (Address, Vec<Val>, Val) =
                events.get(events.len() - 1).unwrap();
            assert_eq!(topics.len(), 2);
            assert_eq!(
                Symbol::try_from_val(env, &topics.get(0).unwrap()).unwrap(),
                Symbol::new(env, "HuntCreated")
            );
            assert_eq!(u64::try_from_val(env, &topics.get(1).unwrap()).unwrap(), hunt_id);
        });
    }

    /// Verifies that a single storage representation underlies both the
    /// public `is_blacklisted` query and the `create_hunt` enforcement path.
    ///
    /// The bug this guards against: before consolidation there were three
    /// independent blacklist stores (`BLKLST_V`, per-address `BLKLST`, and a
    /// `Map<Address,bool>` also named `BLKLST`).  `blacklist_creator` wrote to
    /// the per-address key; `is_creator_blacklisted` (used by `create_hunt`)
    /// read from the Map key.  An admin blacklisting a creator via the public
    /// entry-point would see `is_blacklisted() == true` while `create_hunt`
    /// still succeeded.
    #[test]
    fn test_blacklist_same_storage_for_query_and_enforcement() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let creator = Address::generate(&env);

        with_core_contract(&env, |env, _cid| {
            HuntyCore::initialize_admin(env.clone(), admin.clone()).unwrap();

            // Before blacklisting: query returns false, hunt creation succeeds.
            assert!(!HuntyCore::is_blacklisted(env.clone(), creator.clone()));
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt Before"),
                String::from_str(env, "Should succeed"),
                None,
                None,
                5u32,
                None,
            )
            .expect("create_hunt must succeed before blacklisting");

            // Blacklist the creator via the public admin entry-point.
            HuntyCore::blacklist_creator(env.clone(), admin.clone(), creator.clone()).unwrap();

            // The public query must reflect the new state.
            assert!(
                HuntyCore::is_blacklisted(env.clone(), creator.clone()),
                "is_blacklisted query must return true after blacklisting"
            );

            // create_hunt must be blocked by the *same* storage entry.
            let err = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Blacklisted Hunt"),
                String::from_str(env, "Must not be created"),
                None,
                None,
                5u32,
                None,
            )
            .unwrap_err();
            assert_eq!(
                err,
                HuntErrorCode::AddressBlacklisted,
                "create_hunt must reject a blacklisted creator"
            );

            // After removal: query returns false and creation succeeds again.
            HuntyCore::remove_from_blacklist(env.clone(), admin.clone(), creator.clone()).unwrap();
            assert!(
                !HuntyCore::is_blacklisted(env.clone(), creator.clone()),
                "is_blacklisted must return false after removal"
            );
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt After"),
                String::from_str(env, "Should succeed again"),
                None,
                None,
                5u32,
                None,
            )
            .expect("create_hunt must succeed after removal from blacklist");
        });
    }

    #[test]
    fn test_pause_contract_blocks_registration_until_unpaused() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            HuntyCore::initialize_admin(env.clone(), admin.clone()).unwrap();
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                5u32,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, None).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::pause_contract(env.clone(), admin.clone()).unwrap();

            let err =
                HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap_err();
            assert_eq!(err, HuntErrorCode::ContractPaused);
            assert!(HuntyCore::is_contract_paused(env.clone()));

            HuntyCore::unpause_contract(env.clone(), admin.clone()).unwrap();
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
        });
    }

    #[test]
    fn test_pause_contract_requires_admin() {
        let env = Env::default();
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let attacker = Address::generate(&env);

        let err = with_core_contract(&env, |env, _cid| {
            HuntyCore::initialize_admin(env.clone(), admin.clone()).unwrap();
            HuntyCore::pause_contract(env.clone(), attacker.clone()).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::Unauthorized);
    }

    #[test]
    fn test_register_player_duplicate_fails() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        // Pre-populate storage with existing progress so that the single register_player
        // call hits the duplicate check (mock_all_auths only allows one auth per test frame).
        let err = with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            let current_time = env.ledger().timestamp();
            let existing =
                crate::types::PlayerProgress::new(env, player.clone(), hunt_id, current_time);
            Storage::save_player_progress(env, &existing);

            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::DuplicateRegistration);
    }

    #[test]
    fn test_register_player_allowed_after_reactivation() {
        // A player who registered in a previous activation cycle must be able to
        // re-register after the hunt is deactivated and reactivated.
        let env = Env::default();
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let (hunt_id, core_id) = with_core_contract(&env, |env, cid| {
            env.ledger().set_timestamp(1_000);
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, None).unwrap();
            (hunt_id, cid.clone())
        });

        // First activation
        as_core_contract(&env, &core_id, |env| {
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        // Player registers
        as_core_contract(&env, &core_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
        });

        let first_progress = as_core_contract(&env, &core_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player.clone()).unwrap()
        });

        // Creator deactivates
        as_core_contract(&env, &core_id, |env| {
            HuntyCore::deactivate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        env.ledger().set_timestamp(2_000);

        // Reactivate
        as_core_contract(&env, &core_id, |env| {
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        let hunt = as_core_contract(&env, &core_id, |env| {
            Storage::get_hunt(env, hunt_id).unwrap()
        });
        assert!(first_progress.started_at < hunt.activated_at);

        // Player should be able to register again â€” old progress is stale
        as_core_contract(&env, &core_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
        });

        let latest_progress = as_core_contract(&env, &core_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player.clone()).unwrap()
        });
        assert!(latest_progress.started_at >= hunt.activated_at);
        assert_eq!(latest_progress.completed_clues.len(), 0);

        // But a second call in the same cycle must still be rejected
        let err = as_core_contract(&env, &core_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap_err()
        });
        assert_eq!(err, HuntErrorCode::DuplicateRegistration);
    }

    #[test]
    fn test_register_player_hunt_not_found() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();
        let player = Address::generate(&env);

        let err = with_core_contract(&env, |env, _cid| {
            HuntyCore::register_player(env.clone(), 9999, player.clone()).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::HuntNotFound);
    }

    #[test]
    fn test_register_player_hunt_not_active_draft() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let err = with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            // Hunt is still Draft, not activated
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::InvalidHuntStatus);
    }

    #[test]
    fn test_register_player_hunt_ended() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");
        let end_time = 1_700_000_001; // One second after "now"

        let err = with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                Some(end_time),
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            // Move time past end_time
            env.ledger().set_timestamp(1_700_000_002);
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::HuntNotActive);
    }

    #[test]
    fn test_submit_answer_hunt_ended() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");
        let end_time = 1_700_000_001; // One second after "now"

        let (hunt_id, core_id) = with_core_contract(&env, |env, cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                Some(end_time),
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer.clone(), 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
            (hunt_id, cid.clone())
        });

        // Move time past end_time
        env.ledger().set_timestamp(1_700_000_002);
        env.mock_all_auths();

        let err = as_core_contract(&env, &core_id, |env| {
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player.clone(), answer.clone())
                .unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::HuntNotActive);
    }

    #[test]
    fn test_register_player_multiple_players_same_hunt() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let player1 = Address::generate(&env);
        let player2 = Address::generate(&env);
        let player3 = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();

            HuntyCore::register_player(env.clone(), hunt_id, player1.clone()).unwrap();
            HuntyCore::register_player(env.clone(), hunt_id, player2.clone()).unwrap();
            HuntyCore::register_player(env.clone(), hunt_id, player3.clone()).unwrap();

            let p1 = HuntyCore::get_player_progress(env.clone(), hunt_id, player1.clone()).unwrap();
            let p2 = HuntyCore::get_player_progress(env.clone(), hunt_id, player2.clone()).unwrap();
            let p3 = HuntyCore::get_player_progress(env.clone(), hunt_id, player3.clone()).unwrap();

            assert_eq!(p1.player, player1);
            assert_eq!(p2.player, player2);
            assert_eq!(p3.player, player3);
            assert_eq!(p1.hunt_id, hunt_id);
            assert_eq!(p2.hunt_id, hunt_id);
            assert_eq!(p3.hunt_id, hunt_id);
        });
    }

    #[test]
    #[should_panic]
    fn test_register_player_unauthorized() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        // Do NOT mock auth â€” player.require_auth() will fail if not authorized
        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
        });
    }

    #[test]
    fn test_get_player_progress_not_registered() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let err = with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            // Player never registered
            HuntyCore::get_player_progress(env.clone(), hunt_id, player.clone()).unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::PlayerNotRegistered);
    }

    // ========== Player Progress Query Tests ==========

    #[test]
    fn test_get_player_progress_returns_state_after_submit() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        let contract_id = env.register_contract(None, super::HuntyCore);
        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q1");
        let answer = String::from_str(&env, "a");

        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap()
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(
                env.clone(),
                hunt_id,
                question.clone(),
                answer.clone(),
                10,
                true, 1)
            .unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player.clone(), answer.clone())
                .unwrap();
        });
        let progress = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player.clone()).unwrap()
        });
        assert_eq!(progress.player, player);
        assert_eq!(progress.hunt_id, hunt_id);
        assert_eq!(progress.completed_clues.len(), 1);
        assert_eq!(progress.required_completed_count, 1);
        assert_eq!(progress.total_score, 10);
        assert!(progress.is_completed);
        assert!(progress.completed_at > 0);
    }

    #[test]
    fn test_pause_contract_blocks_answer_submission_until_unpaused() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let admin = Address::generate(&env);
        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let contract_id = env.register_contract(None, super::HuntyCore);
        env.mock_all_auths();
        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::initialize_admin(env.clone(), admin.clone()).unwrap();
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer.clone(), 10, true, 1)
                .unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
            HuntyCore::pause_contract(env.clone(), admin.clone()).unwrap();
            hunt_id
        });

        env.mock_all_auths();
        let err = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(
                env.clone(),
                hunt_id,
                1,
                player.clone(),
                answer.clone(),
            )
            .unwrap_err()
        });
        assert_eq!(err, HuntErrorCode::ContractPaused);

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::unpause_contract(env.clone(), admin.clone()).unwrap();
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player.clone(), answer)
                .unwrap();
        });
    }

    #[test]
    fn test_required_completed_counter_is_not_double_incremented_on_resubmit() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);

        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let contract_id = env.register_contract(None, super::HuntyCore);
        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer.clone(), 10, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            hunt_id
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player.clone(), answer.clone()).unwrap();
        });
        env.mock_all_auths();
        let resubmit = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player.clone(), answer)
        });

        assert_eq!(resubmit, Err(HuntErrorCode::ClueAlreadyCompleted));

        let progress = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player.clone()).unwrap()
        });
        assert_eq!(progress.required_completed_count, 1);
    }


    fn test_required_completed_counter_stays_isolated_per_player() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);

        let creator = Address::generate(&env);
        let player_a = Address::generate(&env);
        let player_b = Address::generate(&env);
        let answer = String::from_str(&env, "a");

        let contract_id = env.register_contract(None, super::HuntyCore);
        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
            )
            .unwrap()
        });

        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(
                env.clone(),
                hunt_id,
                String::from_str(env, "Q1"),
                answer.clone(),
                5,
                true,
                1,
            )
            .unwrap();
        });

        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(
                env.clone(),
                hunt_id,
                String::from_str(env, "Q2"),
                answer.clone(),
                5,
                true,
                1,
            )
            .unwrap();
        });

        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player_a.clone()).unwrap();
            HuntyCore::register_player(env.clone(), hunt_id, player_b.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player_a.clone(), answer.clone())
                .unwrap();
            HuntyCore::submit_answer(env.clone(), hunt_id, 2, player_b.clone(), answer.clone())
                .unwrap();
        });

        let progress_a = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player_a.clone()).unwrap()
        });
        let progress_b = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player_b.clone()).unwrap()
        });

        assert_eq!(progress_a.required_completed_count, 1);
        assert_eq!(progress_b.required_completed_count, 1);
        assert!(!progress_a.is_completed);
        assert!(!progress_b.is_completed);
    }

    #[test]
    fn test_get_completed_clues_empty_when_not_registered() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);
        env.mock_all_auths();

        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let list = with_core_contract(&env, |env, _cid| {
            let hunt_id = HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap();
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer, 1, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
            HuntyCore::get_completed_clues(env.clone(), hunt_id, player.clone())
        });

        assert_eq!(list.len(), 0);
    }

    #[test]
    fn test_get_completed_clues_returns_ids_after_submit() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);

        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let q1 = String::from_str(&env, "Q1");
        let q2 = String::from_str(&env, "Q2");
        let a = String::from_str(&env, "a");

        let contract_id = env.register_contract(None, super::HuntyCore);
        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
                0,
                None,
            )
            .unwrap()
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(env.clone(), hunt_id, q1, a.clone(), 5, false, 1).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(env.clone(), hunt_id, q2.clone(), a.clone(), 10, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player.clone(), a.clone()).unwrap();
        });
        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(env.clone(), hunt_id, 2, player.clone(), a, 1, env.ledger().timestamp()).unwrap();
        });
        let list = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_completed_clues(env.clone(), hunt_id, player.clone())
        });

        assert_eq!(list.len(), 2);
        assert_eq!(list.get(0).unwrap(), 1);
        assert_eq!(list.get(1).unwrap(), 2);
    }

    #[test]
    fn test_submit_answer_clue_already_completed_does_not_double_count_score() {
        let env = Env::default();
        env.ledger().set_timestamp(1_700_000_000);

        let creator = Address::generate(&env);
        let player = Address::generate(&env);
        let question = String::from_str(&env, "Q");
        let answer = String::from_str(&env, "a");

        let contract_id = env.register_contract(None, super::HuntyCore);
        let hunt_id = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::create_hunt(
                env.clone(),
                creator.clone(),
                String::from_str(env, "Hunt"),
                String::from_str(env, "Desc"),
                None,
                None,
            )
            .unwrap()
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::add_clue(env.clone(), hunt_id, question, answer.clone(), 10, true, 1).unwrap();
            HuntyCore::activate_hunt(env.clone(), hunt_id, creator.clone()).unwrap();
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::register_player(env.clone(), hunt_id, player.clone()).unwrap();
        });

        env.mock_all_auths();
        as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player.clone(), answer.clone())
                .unwrap();
        });

        env.mock_all_auths();
        let err = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::submit_answer(env.clone(), hunt_id, 1, player.clone(), answer.clone())
                .unwrap_err()
        });

        assert_eq!(err, HuntErrorCode::ClueAlreadyCompleted);

        let progress = as_core_contract(&env, &contract_id, |env| {
            HuntyCore::get_player_progress(env.clone(), hunt_id, player.clone()).unwrap()
        });

        assert_eq!(progress.completed_clues.len(), 1);
        assert_eq!(progress.total_score, 10);
    }

    #[test]
    fn test_compact_storage_roundtrip() {
        let env = Env::default();
        let player = Address::generate(&env);
        let hunt_id = 42u64;
        let activated_at = 1_700_000_000u64;
        let started_at = 1_700_000_600u64; // 10 minutes delta
        let completed_at = 1_700_003_600u64; // 50 minutes delta from started_at

        // Recreate PlayerProgress structure
        let mut progress = crate::types::PlayerProgress::new(&env, player.clone(), hunt_id, started_at);
        progress.is_completed = true;
        progress.reward_claimed = true;
        progress.completed_at = completed_at;
        progress.total_score = 1000;
        progress.required_completed_count = 5;

        // Record some clues and attempts
        progress.completed_clues.push_back(1);
        progress.completed_clues.push_back(2);
        progress.clue_last_attempts.set(1, 3);
        progress.clue_last_attempts.set(2, 1);

        // Convert to compact stored form
        let stored = progress.to_stored(activated_at);

        // Verify stored compact values
        assert_eq!(stored.started_at_delta, 600);
        assert_eq!(stored.completed_at_delta, 3000);
        assert_eq!(stored.flags, 0b0000_0011u32);
        assert_eq!(stored.total_score, 1000);
        assert_eq!(stored.required_completed_count, 5);

        // Reconstruct from stored
        let restored = crate::types::PlayerProgress::from_stored(&env, stored, player.clone(), hunt_id, activated_at);

        // Verify restored matches original
        assert_eq!(restored.player, player);
        assert_eq!(restored.hunt_id, hunt_id);
        assert_eq!(restored.started_at, started_at);
        assert_eq!(restored.completed_at, completed_at);
        assert_eq!(restored.is_completed, true);
        assert_eq!(restored.reward_claimed, true);
        assert_eq!(restored.total_score, 1000);
        assert_eq!(restored.required_completed_count, 5);
        assert_eq!(restored.completed_clues.len(), 2);
        assert_eq!(restored.completed_clues.get(0).unwrap(), 1);
        assert_eq!(restored.completed_clues.get(1).unwrap(), 2);
        assert_eq!(restored.clue_last_attempts.get(1).unwrap(), 3);
        assert_eq!(restored.clue_last_attempts.get(2).unwrap(), 1);
    }

    #[test]
    fn test_try_get_player_progress_corrupt_data() {
        let env = Env::default();
        let player = Address::generate(&env);
        let hunt_id = 1u64;
        let key = Storage::progress_key(hunt_id, &player);

        // Store corrupt data bytes in progress key that cannot be deserialized as StoredPlayerProgress
        let corrupt_val: Val = soroban_sdk::Symbol::new(&env, "corrupt_data").to_val();
        env.storage().persistent().set(&key, &corrupt_val);

        let result = Storage::try_get_player_progress(&env, hunt_id, &player);
        assert!(result.is_err());
        match result {
            Err(HuntError::CorruptPlayerProgress) => {
                // Payloads stripped; variant still maps to CorruptPlayerProgress
            }
            _ => panic!("Expected CorruptPlayerProgress error"),
        }
    }
}
