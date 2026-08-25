#![no_std]

//! # Hunty Core Contract
//!
//! `hunty-core` manages core protocol parameters, administrative controls,
//! and central registry logic for the Hunty platform on Soroban.

use soroban_sdk::{contract, contractimpl, Address, Env};

/// Contract storage keys for persisting core settings and admin addresses.
#[derive(Clone)]
pub enum DataKey {
    /// Address of the contract administrator.
    Admin,
    /// Pause state flag for emergency controls.
    Paused,
}

#[contract]
pub struct HuntyCoreContract;

#[contractimpl]
impl HuntyCoreContract {
    /// Initializes the core contract with an administrator address.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment instance.
    /// * `admin` - The address granted administrative privileges.
    ///
    /// # Panics
    ///
    /// Panics if the contract has already been initialized.
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic!("already initialized");
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Paused, &false);
    }

    /// Retrieves the current administrator address.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment instance.
    ///
    /// # Returns
    ///
    /// The `Address` of the current contract administrator.
    pub fn get_admin(env: Env) -> Address {
        env.storage()
            .instance()
            .get(&DataKey::Admin)
            .expect("admin not set")
    }

    /// Updates the contract administrator address.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment instance.
    /// * `new_admin` - The address to set as the new administrator.
    ///
    /// # Panics
    ///
    /// Panics if the caller is not authenticated as the current administrator.
    pub fn set_admin(env: Env, new_admin: Address) {
        let admin = Self::get_admin(env.clone());
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
    }

    /// Sets the emergency pause state of the contract.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment instance.
    /// * `paused` - `true` to pause operations, `false` to resume.
    ///
    /// # Panics
    ///
    /// Panics if the caller is not authenticated as the administrator.
    pub fn set_paused(env: Env, paused: bool) {
        let admin = Self::get_admin(env.clone());
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &paused);
    }

    /// Returns the current pause status of the contract.
    ///
    /// # Arguments
    ///
    /// * `env` - The Soroban environment instance.
    ///
    /// # Returns
    ///
    /// `true` if operations are currently paused, `false` otherwise.
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }
}