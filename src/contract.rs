use crate::msg::{
    ExecuteMsg, InstantiateMsg, MigrateMsg, QueryMsg, VestingAccountResponse, VestingData,
    VestingSchedule,
};
use crate::state::{denom_to_key, VestingAccount, VESTED_BY_DENOM, VESTING_ACCOUNTS};
#[cfg(not(feature = "library"))]
use cosmwasm_std::entry_point;
use cosmwasm_std::{
    to_binary, Attribute, BankMsg, Binary, Coin, CosmosMsg, Deps, DepsMut, Env, MessageInfo, Order,
    Response, StdError, StdResult, Uint128,
};
use cw2::set_contract_version;
use cw20::Denom;
use cw_storage_plus::Bound;
use serde_json::to_string;

// version info for migration info
const CONTRACT_NAME: &str = "crates.io:vesting_contract";
const CONTRACT_VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn instantiate(
    deps: DepsMut,
    _env: Env,
    _info: MessageInfo,
    _msg: InstantiateMsg,
) -> StdResult<Response> {
    set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;
    Ok(Response::new())
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn execute(deps: DepsMut, env: Env, info: MessageInfo, msg: ExecuteMsg) -> StdResult<Response> {
    match msg {
        ExecuteMsg::RegisterVestingAccount {
            master_address,
            address,
            vesting_schedule,
        } => {
            // deposit validation
            if info.funds.len() != 1 {
                return Err(StdError::generic_err("must deposit only one type of token"));
            } else if info.funds[0].amount.is_zero() {
                return Err(StdError::generic_err("assert(funds > 0)"));
            }

            let deposit_coin = info.funds[0].clone();

            register_vesting_account(
                deps,
                env,
                master_address,
                address,
                deposit_coin.denom.clone(),
                deposit_coin,
                vesting_schedule,
            )
        }
        ExecuteMsg::DeregisterVestingAccount {
            denom,
            vested_token_recipient,
        } => deregister_vesting_account(deps, env, info, denom, vested_token_recipient),
        ExecuteMsg::Claim { denoms, recipient } => claim(deps, env, info, denoms, recipient),
    }
}

/// Registers a new vesting account.
fn register_vesting_account(
    deps: DepsMut,
    env: Env,
    master_address: String,
    address: String,
    deposit_denom: String,
    deposit: Coin,
    vesting_schedule: VestingSchedule,
) -> StdResult<Response> {
    let deposit_amount = deposit.amount;
    let deposit_denom_str = deposit.denom;
    deps.api.addr_validate(&master_address)?;
    deps.api.addr_validate(&address)?;

    // vesting_account existence check
    if VESTING_ACCOUNTS.has(deps.storage, (address.as_str(), &deposit_denom)) {
        return Err(StdError::generic_err("already exists"));
    }

    // validate vesting schedule
    match vesting_schedule {
        VestingSchedule::LinearVesting {
            start_time,
            end_time,
            vesting_amount,
        } => {
            if vesting_amount != deposit_amount {
                return Err(StdError::generic_err(
                    "assert(deposit_amount == vesting_amount)",
                ));
            }

            if start_time < env.block.time.seconds() {
                return Err(StdError::generic_err("assert(start_time < block_time)"));
            }

            if end_time <= start_time {
                return Err(StdError::generic_err("assert(end_time <= start_time)"));
            }
        }
        VestingSchedule::PeriodicVesting {
            start_time,
            end_time,
            vesting_interval,
            amount,
        } => {
            if amount.is_zero() {
                return Err(StdError::generic_err(
                    "cannot make zero token vesting account",
                ));
            }

            if start_time < env.block.time.seconds() {
                return Err(StdError::generic_err("invalid start_time"));
            }

            if end_time <= start_time {
                return Err(StdError::generic_err("assert(end_time > start_time)"));
            }

            if vesting_interval == 0 {
                return Err(StdError::generic_err("assert(vesting_interval != 0)"));
            }

            let time_period = end_time - start_time;
            if time_period != (time_period / vesting_interval) * vesting_interval {
                return Err(StdError::generic_err(
                    "assert((end_time - start_time) % vesting_interval == 0)",
                ));
            }

            let num_interval = time_period / vesting_interval;
            let vesting_amount = amount.checked_mul(Uint128::from(num_interval))?;
            if vesting_amount != deposit_amount {
                return Err(StdError::generic_err(
                    "assert(deposit_amount = amount * ((end_time - start_time) / vesting_interval))",
                ));
            }
        }
    }

    VESTING_ACCOUNTS.save(
        deps.storage,
        (address.as_str(), &deposit_denom_str),
        &VestingAccount {
            master_address: master_address.clone(),
            address: address.to_string(),
            vesting_denom: deposit_denom.clone(),
            vesting_amount: deposit_amount,
            vesting_schedule,
            claimed_amount: Uint128::zero(),
        },
    )?;

    let total_vested = match VESTED_BY_DENOM.may_load(deps.storage, &deposit_denom_str)? {
        Some(data) => data,
        None => Uint128::new(0),
    };
    VESTED_BY_DENOM.save(
        deps.storage,
        &deposit_denom_str,
        &(deposit_amount + total_vested),
    )?;

    Ok(Response::new().add_attributes(vec![
        ("action", "register_vesting_account"),
        ("master_address", master_address.as_str()),
        ("address", address.as_str()),
        ("vesting_denom", &to_string(&deposit_denom).unwrap()),
        ("vesting_amount", &deposit_amount.to_string()),
    ]))
}

fn deregister_vesting_account(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    denom: String,
    vested_token_recipient: Option<String>,
) -> StdResult<Response> {
    if !info.funds.is_empty() {
        return Err(StdError::GenericErr {
            msg: String::from("Funds not allowed."),
        });
    };
    let sender = info.sender;

    let mut messages: Vec<CosmosMsg> = vec![];

    // vesting_account existence check
    let account = VESTING_ACCOUNTS.may_load(deps.storage, (sender.as_str(), &denom))?;
    if account.is_none() {
        return Err(StdError::generic_err(format!(
            "vesting entry is not found for denom {:?}",
            to_string(&denom).unwrap(),
        )));
    }

    let account = account.unwrap();
    let master_account = account.master_address;
    // remove vesting account
    VESTING_ACCOUNTS.remove(deps.storage, (sender.as_str(), &denom));

    let vested_amount = account
        .vesting_schedule
        .vested_amount(env.block.time.seconds())?;
    let claimed_amount = account.claimed_amount;

    // transfer already vested but not claimed amount to
    // a account address or the given `vested_token_recipient` address
    let claimable_amount = vested_amount.checked_sub(claimed_amount)?;
    if !claimable_amount.is_zero() {
        let recipient = vested_token_recipient.unwrap_or_else(|| sender.to_string());
        deps.api.addr_validate(&recipient)?;

        let message: CosmosMsg = BankMsg::Send {
            to_address: recipient,
            amount: vec![Coin {
                denom: account.vesting_denom.clone(),
                amount: claimable_amount,
            }],
        }
        .into();

        messages.push(message);
    }

    // transfer left vesting amount to owner or
    // the given `left_vesting_token_recipient` address
    let left_vesting_amount = account.vesting_amount.checked_sub(vested_amount)?;
    if !left_vesting_amount.is_zero() {
        let recipient = master_account;
        deps.api.addr_validate(&recipient)?;
        let message: CosmosMsg = BankMsg::Send {
            to_address: recipient,
            amount: vec![Coin {
                denom: account.vesting_denom.clone(),
                amount: left_vesting_amount,
            }],
        }
        .into();

        messages.push(message);
    }

    let total_vested = match VESTED_BY_DENOM.may_load(deps.storage, &denom)? {
        Some(data) => data,
        None => Uint128::new(0),
    };
    VESTED_BY_DENOM.save(deps.storage, &denom, &(total_vested - left_vesting_amount))?;

    Ok(Response::new().add_messages(messages).add_attributes(vec![
        ("action", "deregister_vesting_account"),
        ("address", sender.as_str()),
        ("vesting_denom", &to_string(&account.vesting_denom).unwrap()),
        ("vesting_amount", &account.vesting_amount.to_string()),
        ("vested_amount", &vested_amount.to_string()),
        ("left_vesting_amount", &left_vesting_amount.to_string()),
    ]))
}

fn claim(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    denoms: Vec<String>,
    recipient: Option<String>,
) -> StdResult<Response> {
    if !info.funds.is_empty() {
        return Err(StdError::GenericErr {
            msg: "Funds not allowed.".to_string(),
        });
    };

    let sender = info.sender;
    let recipient = recipient.unwrap_or_else(|| sender.to_string());
    deps.api.addr_validate(&recipient)?;

    let mut messages: Vec<CosmosMsg> = vec![];
    let mut attrs: Vec<Attribute> = vec![];
    for denom in denoms.iter() {
        // vesting_account existence check
        let account = VESTING_ACCOUNTS.may_load(deps.storage, (sender.as_str(), denom))?;
        if account.is_none() {
            return Err(StdError::generic_err(format!(
                "vesting entry is not found for denom {}",
                to_string(&denom).unwrap(),
            )));
        }

        let mut account = account.unwrap();
        let vested_amount = account
            .vesting_schedule
            .vested_amount(env.block.time.seconds())?;
        let claimed_amount = account.claimed_amount;

        let claimable_amount = vested_amount.checked_sub(claimed_amount)?;
        if claimable_amount.is_zero() {
            continue;
        }

        account.claimed_amount = vested_amount;
        if account.claimed_amount == account.vesting_amount {
            VESTING_ACCOUNTS.remove(deps.storage, (sender.as_str(), denom));
        } else {
            VESTING_ACCOUNTS.save(deps.storage, (sender.as_str(), denom), &account)?;
        }

        let message: CosmosMsg = BankMsg::Send {
            to_address: recipient.clone(),
            amount: vec![Coin {
                denom: account.vesting_denom.clone(),
                amount: claimable_amount,
            }],
        }
        .into();
        messages.push(message);
        attrs.extend(
            vec![
                Attribute::new("vesting_denom", &to_string(&account.vesting_denom).unwrap()),
                Attribute::new("vesting_amount", &account.vesting_amount.to_string()),
                Attribute::new("vested_amount", &vested_amount.to_string()),
                Attribute::new("claim_amount", &claimable_amount.to_string()),
            ]
            .into_iter(),
        );

        let total_vested = VESTED_BY_DENOM.may_load(deps.storage, denom)?;

        if total_vested.is_none() {
            return Err(StdError::generic_err("already exists"));
        };

        VESTED_BY_DENOM.save(
            deps.storage,
            denom,
            &(total_vested.unwrap() - claimable_amount),
        )?;
    }

    Ok(Response::new()
        .add_messages(messages)
        .add_attributes(vec![("action", "claim"), ("address", sender.as_str())])
        .add_attributes(attrs))
}

#[cfg_attr(not(feature = "library"), entry_point)]
pub fn query(deps: Deps, env: Env, msg: QueryMsg) -> StdResult<Binary> {
    match msg {
        QueryMsg::VestingAccount {
            address,
            start_after,
            limit,
        } => to_binary(&vesting_account(deps, env, address, start_after, limit)?),
        QueryMsg::VestedTokens { denom } => to_binary(&vested_tokens(deps, env, denom)?),
    }
}

const MAX_LIMIT: u32 = 30;
const DEFAULT_LIMIT: u32 = 10;
fn vesting_account(
    deps: Deps,
    env: Env,
    address: String,
    start_after: Option<Denom>,
    limit: Option<u32>,
) -> StdResult<VestingAccountResponse> {
    let mut vestings: Vec<VestingData> = vec![];
    let limit = limit.unwrap_or(DEFAULT_LIMIT).min(MAX_LIMIT) as usize;
    deps.api.addr_validate(&address)?;

    for item in VESTING_ACCOUNTS
        .prefix(address.as_str())
        .range(
            deps.storage,
            start_after
                .map(denom_to_key)
                .map(|v| v.as_bytes().to_vec())
                .map(Bound::ExclusiveRaw),
            None,
            Order::Ascending,
        )
        .take(limit)
    {
        let (_, account) = item?;
        let vested_amount = account
            .vesting_schedule
            .vested_amount(env.block.time.seconds())?;

        vestings.push(VestingData {
            master_address: account.master_address,
            vesting_denom: account.vesting_denom,
            vesting_amount: account.vesting_amount,
            vested_amount,
            vesting_schedule: account.vesting_schedule,
            claimable_amount: vested_amount.checked_sub(account.claimed_amount)?,
        })
    }

    Ok(VestingAccountResponse { address, vestings })
}

fn vested_tokens(deps: Deps, _env: Env, denom: String) -> StdResult<Uint128> {
    let total_vested = match VESTED_BY_DENOM.may_load(deps.storage, &denom)? {
        Some(data) => data,
        None => Uint128::new(0),
    };
    Ok(total_vested)
}

#[entry_point]
pub fn migrate(deps: DepsMut, _env: Env, _msg: MigrateMsg) -> Result<Response, StdError> {
    let ver = cw2::get_contract_version(deps.storage)?;
    // ensure we are migrating from an allowed contract
    if ver.contract != CONTRACT_NAME {
        return Err(StdError::generic_err("Can only upgrade from same type"));
    }
    // note: better to do proper semver compare, but string compare *usually* works
    if ver.version.as_str() > CONTRACT_VERSION {
        return Err(StdError::generic_err("Cannot upgrade from a newer version"));
    }

    // set the new version
    cw2::set_contract_version(deps.storage, CONTRACT_NAME, CONTRACT_VERSION)?;

    // do any desired state migrations...

    Ok(Response::default())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cosmwasm_std::testing::{mock_dependencies, mock_env, mock_info};
    use cosmwasm_std::{coins, Addr, CosmosMsg, StdError, Timestamp};

    const DENOM: &str = "TKN";

    #[test]
    fn proper_initialization() {
        let env = mock_env();
        let mut deps = mock_dependencies();
        let info = mock_info("sender", &coins(0, DENOM.to_string()));

        let msg = InstantiateMsg {};
        let res = instantiate(deps.as_mut(), env.clone(), info.clone(), msg).unwrap();
        assert_eq!(res.messages.len(), 0);
        assert_eq!(res.attributes.len(), 0);
    }

    #[test]
    fn register_vesting_account_linear_invalid_request() {
        // Mock dependencies
        let mut env = mock_env();
        let mut deps = mock_dependencies();

        // vesting details
        let vesting_amount = 1000;
        let address = Addr::unchecked("user1");
        let vesting_schedule = VestingSchedule::LinearVesting {
            start_time: 5000,
            end_time: 6000,
            vesting_amount: Uint128::from(vesting_amount),
        };

        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: String::new(),
            address: address.to_string(),
            vesting_schedule,
        };

        let info = mock_info(address.as_str(), &coins(vesting_amount, DENOM));

        // * FAIL for empty master address
        let res = execute(deps.as_mut(), env.clone(), info.clone(), msg).unwrap_err();
        match res {
            StdError::GenericErr { .. } => {}
            e => panic!("{:?}", e),
        };

        // * FAIL for start_time < block_time
        let vesting_schedule = VestingSchedule::LinearVesting {
            start_time: 5000,
            end_time: 6000,
            vesting_amount: Uint128::from(vesting_amount),
        };
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: String::from("master"),
            address: address.to_string(),
            vesting_schedule,
        };

        env.block.time = Timestamp::from_seconds(6000);

        let result = execute(deps.as_mut(), env.clone(), info.clone(), msg).unwrap_err();
        match result {
            StdError::GenericErr { msg }
                if msg == String::from("assert(start_time < block_time)") => {}
            e => panic!("{:?}", e),
        };

        // * FAIL for start_time == end_time
        let vesting_schedule = VestingSchedule::LinearVesting {
            start_time: 6000,
            end_time: 6000,
            vesting_amount: Uint128::from(vesting_amount),
        };

        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: String::from("master"),
            address: address.to_string(),
            vesting_schedule,
        };

        let result = execute(deps.as_mut(), env.clone(), info.clone(), msg).unwrap_err();
        match result {
            StdError::GenericErr { msg }
                if msg == String::from("assert(end_time <= start_time)") => {}
            e => panic!("{:?}", e),
        };

        // * FAIL for empty `address` field.
        let vesting_schedule = VestingSchedule::LinearVesting {
            start_time: 6000,
            end_time: 8000,
            vesting_amount: Uint128::from(vesting_amount),
        };

        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: String::from("master"),
            address: String::new(),
            vesting_schedule,
        };

        execute(deps.as_mut(), env.clone(), info.clone(), msg).unwrap_err();
    }

    #[test]
    fn register_vesting_account_periodic_invalid_request() {
        // Mock dependencies
        let mut env = mock_env();
        let mut deps = mock_dependencies();

        env.block.time = Timestamp::from_seconds(200);

        // vesting details
        let vesting_amount = 1000;
        let address = Addr::unchecked("user1");
        let vesting_schedule = VestingSchedule::PeriodicVesting {
            start_time: 1000,
            end_time: 5000,
            vesting_interval: 1000,
            amount: Uint128::from(vesting_amount),
        };

        // * FAIL for sending excess amount
        let deposit_amount = 4000 / 1000 * vesting_amount + 1000;
        let info = mock_info(address.as_str(), &coins(deposit_amount, DENOM));
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: "master".to_string(),
            address: address.to_string(),
            vesting_schedule: vesting_schedule.clone(),
        };
        let result = execute(deps.as_mut(), env.clone(), info, msg).unwrap_err();
        match result {
            StdError::GenericErr { msg } if msg == "assert(deposit_amount = amount * ((end_time - start_time) / vesting_interval))" => {}
            e => panic!("{:?}", e),
        };
    }

    #[test]
    fn register_vesting_account_periodic_valid_request() {
        let mut env = mock_env();
        let mut deps = mock_dependencies();

        env.block.time = Timestamp::from_seconds(200);

        // vesting details
        let vesting_amount = 1000;
        let address = Addr::unchecked("user1");
        let vesting_schedule = VestingSchedule::PeriodicVesting {
            start_time: 1000,
            end_time: 5000,
            vesting_interval: 1000,
            amount: Uint128::from(vesting_amount),
        };

        // PASS
        let deposit_amount = 4000 / 1000 * vesting_amount;
        let info = mock_info("user1", &coins(deposit_amount, DENOM));
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: String::from("master"),
            address: address.to_string(),
            vesting_schedule: vesting_schedule.clone(),
        };
        let result = execute(deps.as_mut(), env.clone(), info, msg).unwrap();
        assert_eq!(result.messages.len(), 0);

        // Check correct update in VESTING_ACCOUNTS
        let vesting_account = VESTING_ACCOUNTS
            .load(deps.as_ref().storage, (address.as_str(), DENOM))
            .unwrap();
        assert_eq!(vesting_account.address, address.to_string());
        assert_eq!(vesting_account.master_address, "master".to_string());
        assert_eq!(
            vesting_account.vesting_amount,
            Uint128::from(deposit_amount)
        );
        assert_eq!(vesting_account.claimed_amount, Uint128::zero());
        assert_eq!(vesting_account.vesting_denom, DENOM.to_string());
        assert_eq!(vesting_account.vesting_schedule, vesting_schedule);

        // Check correct update in VESTED_BY_DENOM
        let denom_vested = VESTED_BY_DENOM.load(deps.as_ref().storage, DENOM).unwrap();
        assert_eq!(denom_vested, Uint128::from(deposit_amount));
    }

    #[test]
    fn register_vesting_account_linear_valid_request() {
        let mut env = mock_env();
        let mut deps = mock_dependencies();

        env.block.time = Timestamp::from_seconds(1000);

        // vesting details
        let vesting_amount = 100;
        let address = Addr::unchecked("user1");
        let vesting_schedule = VestingSchedule::LinearVesting {
            start_time: 1200,
            end_time: 1500,
            vesting_amount: Uint128::from(vesting_amount),
        };

        // PASS
        let info = mock_info(address.as_str(), &coins(vesting_amount, DENOM));
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: "master".to_string(),
            address: info.sender.clone().to_string(),
            vesting_schedule: vesting_schedule.clone(),
        };

        let res = execute(deps.as_mut(), env.clone(), info.clone(), msg.clone()).unwrap();
        assert_eq!(res.messages.len(), 0);

        // Check correct update in VESTING_ACCOUNTS
        let vesting_account = VESTING_ACCOUNTS
            .load(deps.as_ref().storage, (address.as_str(), DENOM))
            .unwrap();
        assert_eq!(vesting_account.address, address.to_string());
        assert_eq!(vesting_account.claimed_amount, Uint128::zero());
        assert_eq!(vesting_account.master_address, "master".to_string());
        assert_eq!(
            vesting_account.vesting_amount,
            Uint128::from(vesting_amount)
        );
        assert_eq!(vesting_account.vesting_denom, DENOM.to_string());
        assert_eq!(vesting_account.vesting_schedule, vesting_schedule);

        // Check correct update in VESTED_BY_DENOM
        let denom_vested = VESTED_BY_DENOM.load(deps.as_ref().storage, DENOM).unwrap();
        assert_eq!(denom_vested, Uint128::from(vesting_amount));

        // Should return Response
        assert_eq!(
            res,
            Response::new().add_attributes(vec![
                ("action", "register_vesting_account"),
                ("master_address", "master"),
                ("address", info.sender.as_str()),
                ("vesting_denom", &to_string(DENOM).unwrap()),
                ("vesting_amount", &info.funds[0].amount.to_string()),
            ])
        )
    }

    fn create_vesting_account(
        deps: DepsMut,
        env: Env,
        info: MessageInfo,
        start_time: Option<u64>,
        end_time: Option<u64>,
        vesting_amount: Option<Uint128>,
    ) {
        let start_time = start_time.unwrap_or(1000);
        let end_time = end_time.unwrap_or(1500);
        let vesting_amount = vesting_amount.unwrap_or(Uint128::from(1000u64));
        let vesting_schedule = VestingSchedule::LinearVesting {
            start_time,
            end_time,
            vesting_amount,
        };
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: "master".to_string(),
            address: info.sender.to_string(),
            vesting_schedule,
        };
        execute(deps, env, info, msg).unwrap();
    }

    #[test]
    fn deregister_vesting_account_invalid_request() {
        let mut env = mock_env();
        let mut deps = mock_dependencies();

        env.block.time = Timestamp::from_seconds(1000);

        let address = Addr::unchecked("user1");

        // * FAIL: no vesting account present
        let info = mock_info(address.as_str(), &[]);
        let msg = ExecuteMsg::DeregisterVestingAccount {
            denom: DENOM.to_string(),
            vested_token_recipient: None,
        };

        let result = execute(deps.as_mut(), env.clone(), info, msg).unwrap_err();
        match result {
            StdError::GenericErr { .. } => {}
            e => panic!("{:?}", e),
        };

        // * FAIL: Invalid recipient address
        let recipient = String::from("");

        // Create a new vesting account for user1
        let info = mock_info(address.as_str(), &coins(1000, DENOM));
        create_vesting_account(deps.as_mut(), env.clone(), info, None, None, None);

        assert!(VESTING_ACCOUNTS.has(deps.as_ref().storage, (address.as_str(), DENOM)));
        assert!(!VESTING_ACCOUNTS.has(deps.as_ref().storage, (&recipient, DENOM)));

        // Forward the time to 2000, so that all the tokens have vested.
        env.block.time = Timestamp::from_seconds(2000);

        // Deregister the vesting account for user1
        let info = mock_info(address.as_str(), &[]);
        let msg = ExecuteMsg::DeregisterVestingAccount {
            denom: DENOM.to_string(),
            vested_token_recipient: Some(recipient),
        };

        let result = execute(deps.as_mut(), env.clone(), info, msg.clone()).unwrap_err();
        match result {
            StdError::GenericErr { msg }
                if msg == "Invalid input: human address too short".to_string() => {}
            e => panic!("{:?}", e),
        };

        // * FAIL: funds not allowed.
        let info = mock_info("sender", &coins(10, DENOM));
        let result = execute(deps.as_mut(), env.clone(), info, msg).unwrap_err();
        match result {
            StdError::GenericErr { msg } if msg == "Funds not allowed." => {}
            e => panic!("{:?}", e),
        };
    }

    #[test]
    fn claim_invalid_request() {
        let mut env = mock_env();
        let mut deps = mock_dependencies();

        env.block.time = Timestamp::from_seconds(1000);

        let address = Addr::unchecked("user1");

        // * FAIL: incorrect denoms being claimed
        let info = mock_info(address.as_str(), &coins(1000, DENOM));
        create_vesting_account(deps.as_mut(), env.clone(), info, None, None, None);

        let msg = ExecuteMsg::Claim {
            denoms: vec!["DNM".to_string()],
            recipient: None,
        };

        let info = mock_info(address.as_str(), &[]);
        let result = execute(deps.as_mut(), env.clone(), info, msg).unwrap_err();
        match result {
            StdError::GenericErr { msg }
                if msg == "vesting entry is not found for denom \"DNM\"" => {}
            e => panic!("{:?}", e),
        };

        // FAIL: No vested tokens
        let msg = ExecuteMsg::Claim {
            denoms: vec![DENOM.to_string()],
            recipient: None,
        };

        let info = mock_info(address.as_str(), &[]);
        let result = execute(deps.as_mut(), env.clone(), info, msg).unwrap();
        assert_eq!(result.messages.len(), 0);

        // FAIL: Funds not allowed
        let info = mock_info(address.as_str(), &coins(10, DENOM));
        let msg = ExecuteMsg::Claim {
            denoms: vec![DENOM.to_string()],
            recipient: None,
        };

        let result = execute(deps.as_mut(), env.clone(), info, msg).unwrap_err();
        match result {
            StdError::GenericErr { msg } if msg == "Funds not allowed." => {}
            e => panic!("{:?}", e),
        };
    }

    // Test case for claim
    #[test]
    fn testing_claim() {
        let env = mock_env();
        let mut deps = mock_dependencies();
        let info = mock_info("sender", &coins(100, DENOM.to_string()));
        let amount: u64 = 100;
        // registering Message
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: info.sender.to_string(),
            address: info.sender.clone().into_string(),
            vesting_schedule: VestingSchedule::LinearVesting {
                start_time: 1662824814,
                end_time: 1662824914,
                vesting_amount: Uint128::from(amount),
            },
        };
        // Registering the account

        let _res = execute(deps.as_mut(), env.clone(), info.clone(), msg.clone());
        let deposit_denom = info.funds[0].denom.clone();
        let receiver_info = mock_info("recipent", &coins(100, DENOM.to_string()));

        let info = mock_info("sender", &[]);
        let res = claim(
            deps.as_mut(),
            env,
            info.clone(),
            vec![deposit_denom],
            Some(receiver_info.sender.clone().into_string()),
        );

        let messages: Vec<CosmosMsg> = vec![];
        // Should return Respose
        assert_eq!(
            res,
            Ok(Response::new()
                .add_messages(messages)
                .add_attributes(vec![("action", "claim"), ("address", info.sender.as_str())]))
        );
        // .add_attributes(attrs)))
    }

    #[test]
    fn vesting_account_query() {
        let mut env = mock_env();
        let mut deps = mock_dependencies();

        env.block.time = Timestamp::from_seconds(1000);

        const DENOM2: &str = "TKN2";
        let address = Addr::unchecked("user1");
        let vesting_amount = 1000;

        // A simple testcase for the vesting_account fn.

        let info = mock_info(address.as_str(), &coins(vesting_amount, DENOM));
        create_vesting_account(
            deps.as_mut(),
            env.clone(),
            info,
            None,
            None,
            Some(Uint128::from(vesting_amount)),
        );

        let vesting_schedule = VestingSchedule::LinearVesting {
            start_time: 1000,
            end_time: 1500,
            vesting_amount: Uint128::from(vesting_amount),
        };
        let result =
            vesting_account(deps.as_ref(), env.clone(), address.to_string(), None, None).unwrap();
        assert_eq!(result.address, address.to_string());
        assert_eq!(
            result.vestings[0],
            VestingData {
                master_address: "master".to_string(),
                vesting_denom: DENOM.to_string(),
                vesting_amount: Uint128::from(vesting_amount),
                vested_amount: Uint128::zero(),
                vesting_schedule: vesting_schedule.clone(),
                claimable_amount: Uint128::zero()
            }
        );

        let info = mock_info(address.as_str(), &coins(vesting_amount, DENOM2));
        create_vesting_account(
            deps.as_mut(),
            env.clone(),
            info,
            None,
            None,
            Some(Uint128::from(vesting_amount)),
        );

        let result =
            vesting_account(deps.as_ref(), env.clone(), address.to_string(), None, None).unwrap();
        assert_eq!(result.vestings.len(), 2);
        assert_eq!(
            result.vestings[1],
            VestingData {
                master_address: "master".to_string(),
                vesting_amount: Uint128::from(vesting_amount),
                vesting_denom: DENOM2.to_string(),
                vesting_schedule: vesting_schedule,
                vested_amount: Uint128::zero(),
                claimable_amount: Uint128::zero(),
            }
        );
    }

    // testcase for Query to get vesting account
    #[test]
    fn testing_vesting_account() {
        let env = mock_env();
        let mut deps = mock_dependencies();
        let info = mock_info("sender", &coins(100, DENOM.to_string()));
        let amount: u64 = 100;
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: info.sender.to_string(),
            address: info.sender.clone().into_string(),
            vesting_schedule: VestingSchedule::LinearVesting {
                start_time: 1662824814,
                end_time: 1662824914,
                vesting_amount: Uint128::from(amount),
            },
        };
        // Registering the Account
        let _res = execute(deps.as_mut(), env.clone(), info.clone(), msg.clone());
        let deposit_denom = Denom::Native(info.funds[0].denom.clone());
        // Query Message
        let _querymsg = QueryMsg::VestingAccount {
            address: info.sender.to_string(),
            start_after: Some(deposit_denom.clone()),
            limit: Some(0),
        };
        // running Query function
        let res = vesting_account(
            deps.as_ref(),
            env,
            info.sender.clone().into_string(),
            Some(deposit_denom),
            Some(0),
        )
        .unwrap();
        // Should return VEstingAccountrespose.
        assert_eq!(
            res,
            VestingAccountResponse {
                address: info.sender.into_string(),
                vestings: vec![]
            }
        )
    }

    // test case for query to get vesting Tokens.
    #[test]
    fn testing_vesting_tokens() {
        let env = mock_env();
        let mut deps = mock_dependencies();
        let info = mock_info("sender", &coins(100, DENOM.to_string()));
        let amount: u64 = 100;
        // register Message
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: info.sender.to_string(),
            address: info.sender.clone().into_string(),
            vesting_schedule: VestingSchedule::LinearVesting {
                start_time: 1662824814,
                end_time: 1662824914,
                vesting_amount: Uint128::from(amount),
            },
        };

        // Registering Accounts.
        let _res = execute(deps.as_mut(), env.clone(), info.clone(), msg.clone());
        let _deposit_denom = Denom::Native(info.funds[0].denom.clone());

        // Running VestedTokens Query
        let res = vested_tokens(deps.as_ref(), env, info.funds[0].denom.clone()).unwrap();
        assert_eq!(res, Uint128::from(amount));
    }
}
