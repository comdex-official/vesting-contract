use cosmwasm_std::{Addr, StdResult, Uint128};
use cw20::Denom;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct InstantiateMsg {}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ExecuteMsg {
    //////////////////////////
    /// Creator Operations ///
    //////////////////////////
    RegisterVestingAccount {
        master_address: String,
        address: String,
        vesting_schedule: VestingSchedule,
    },

    /// Deregister vesting account for the (sender, denom) pair.
    DeregisterVestingAccount {
        denom: String,
        vested_token_recipient: Option<String>,
    },

    ////////////////////////
    /// VestingAccount Operations ///
    ////////////////////////
    Claim {
        denoms: Vec<String>,
        recipient: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum QueryMsg {
    VestingAccount {
        address: String,
        start_after: Option<Denom>,
        limit: Option<u32>,
    },
    VestedTokens {
        denom: String,
    },
}

#[derive(Serialize, Deserialize, JsonSchema, PartialEq, Debug)]
pub struct VestingAccountResponse {
    pub address: String,
    pub vestings: Vec<VestingData>,
}

#[derive(Serialize, Deserialize, JsonSchema, PartialEq, Debug)]
pub struct VestingData {
    pub master_address: String,
    pub vesting_denom: String,
    pub vesting_amount: Uint128,
    pub vested_amount: Uint128,
    pub vesting_schedule: VestingSchedule,
    pub claimable_amount: Uint128,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VestingSchedule {
    /// LinearVesting is used to vest tokens linearly during a time period.
    /// The total_amount will be vested during this period.
    LinearVesting {
        start_time: u64,         // vesting start time in second unit
        end_time: u64,           // vesting end time in second unit
        vesting_amount: Uint128, // total vesting amount
    },
    /// PeriodicVesting is used to vest tokens
    /// at regular intervals for a specific period.
    /// To minimize calculation error,
    /// (end_time - start_time) should be multiple of vesting_interval
    /// deposit_amount = amount * ((end_time - start_time) / vesting_interval + 1)
    PeriodicVesting {
        start_time: u64,       // vesting start time in second unit
        end_time: u64,         // vesting end time in second unit
        vesting_interval: u64, // vesting interval in second unit
        amount: Uint128,       // the amount will be vested in a interval
    },
}

impl VestingSchedule {
    pub fn vested_amount(&self, block_time: u64) -> StdResult<Uint128> {
        match self {
            VestingSchedule::LinearVesting {
                start_time,
                end_time,
                vesting_amount,
            } => {
                let start_time = start_time;
                let end_time = end_time;

                if &block_time <= start_time {
                    return Ok(Uint128::zero());
                }

                if &block_time >= end_time {
                    return Ok(*vesting_amount);
                }

                let vested_token = vesting_amount
                    .checked_mul(Uint128::from(block_time - start_time))?
                    .checked_div(Uint128::from(end_time - start_time))?;

                Ok(vested_token)
            }
            VestingSchedule::PeriodicVesting {
                start_time,
                end_time,
                vesting_interval,
                amount,
            } => {
                let start_time = start_time;
                let end_time = end_time;

                if &block_time <= start_time {
                    return Ok(Uint128::zero());
                }

                let num_interval = (end_time - start_time) / vesting_interval;
                if &block_time >= end_time {
                    return Ok(amount.checked_mul(Uint128::from(num_interval))?);
                }

                let passed_interval = (block_time - start_time) / vesting_interval;
                Ok(amount.checked_mul(Uint128::from(passed_interval))?)
            }
        }
    }
}

#[test]
fn periodic_vesting_vested_amount_hack() {
    let schedule = VestingSchedule::PeriodicVesting {
        start_time: 105,
        end_time: 110,
        vesting_interval: 5,
        amount: Uint128::new(500000u128),
    };
    assert_eq!(schedule.vested_amount(100).unwrap(), Uint128::zero());
    //FAILS. Got the first tranche at the start_time
    assert_eq!(schedule.vested_amount(105).unwrap(), Uint128::zero());
    //FAILS. Got the first tranche at the start_time
    assert_eq!(schedule.vested_amount(106).unwrap(), Uint128::zero());
    //FAILS. Got double of the intended amount
    assert_eq!(
        schedule.vested_amount(110).unwrap(),
        Uint128::new(500000u128)
    );
}

#[test]
fn linear_vesting_vested_amount() {
    let schedule = VestingSchedule::LinearVesting {
        start_time: 100,
        end_time: 110,
        vesting_amount: Uint128::new(1000000u128),
    };

    assert_eq!(schedule.vested_amount(100).unwrap(), Uint128::zero());
    assert_eq!(
        schedule.vested_amount(105).unwrap(),
        Uint128::new(500000u128)
    );
    assert_eq!(
        schedule.vested_amount(110).unwrap(),
        Uint128::new(1000000u128)
    );
    assert_eq!(
        schedule.vested_amount(115).unwrap(),
        Uint128::new(1000000u128)
    );
}

#[test]
fn periodic_vesting_vested_amount() {
    let schedule = VestingSchedule::PeriodicVesting {
        start_time: 105,
        end_time: 110,
        vesting_interval: 5,
        amount: Uint128::new(500000u128),
    };

    assert_eq!(schedule.vested_amount(100).unwrap(), Uint128::zero());
    assert_eq!(schedule.vested_amount(105).unwrap(), Uint128::zero());
    assert_eq!(
        schedule.vested_amount(110).unwrap(),
        Uint128::new(500000u128)
    );
    assert_eq!(
        schedule.vested_amount(115).unwrap(),
        Uint128::new(500000u128)
    );
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SudoMsg {
    UpdateVestingContract { address: Addr },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, JsonSchema)]
pub struct MigrateMsg {}
