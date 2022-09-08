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
            address,
            denom,
            vested_token_recipient,
            left_vesting_token_recipient,
        } => deregister_vesting_account(
            deps,
            env,
            info,
            address,
            denom,
            vested_token_recipient,
            left_vesting_token_recipient,
        ),
        ExecuteMsg::Claim { denoms, recipient } => claim(deps, env, info, denoms, recipient),
    }
}

fn register_vesting_account(
    deps: DepsMut,
    env: Env,
    master_address: Option<String>,
    address: String,
    deposit_denom: String,
    deposit: Coin,
    vesting_schedule: VestingSchedule,
) -> StdResult<Response> {
    let deposit_amount = deposit.amount;
    let deposit_denom_str = deposit.denom;
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
            if vesting_amount.is_zero() {
                return Err(StdError::generic_err("assert(vesting_amount > 0)"));
            }

            if start_time < env.block.time.seconds() {
                return Err(StdError::generic_err("assert(start_time < block_time)"));
            }

            if end_time <= start_time {
                return Err(StdError::generic_err("assert(end_time <= start_time)"));
            }

            if vesting_amount != deposit_amount {
                return Err(StdError::generic_err(
                    "assert(deposit_amount == vesting_amount)",
                ));
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
                    "assert(deposit_amount = amount * ((end_time - start_time) / vesting_interval + 1))",
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
        (
            "master_address",
            master_address.unwrap_or_default().as_str(),
        ),
        ("address", address.as_str()),
        ("vesting_denom", &to_string(&deposit_denom).unwrap()),
        ("vesting_amount", &deposit_amount.to_string()),
    ]))
}

fn deregister_vesting_account(
    deps: DepsMut,
    env: Env,
    info: MessageInfo,
    address: String,
    denom: String,
    vested_token_recipient: Option<String>,
    left_vesting_token_recipient: Option<String>,
) -> StdResult<Response> {
    let sender = info.sender;

    let mut messages: Vec<CosmosMsg> = vec![];

    // vesting_account existence check
    let account = VESTING_ACCOUNTS.may_load(deps.storage, (address.as_str(), &denom))?;
    if account.is_none() {
        return Err(StdError::generic_err(format!(
            "vesting entry is not found for denom {:?}",
            to_string(&denom).unwrap(),
        )));
    }

    let account = account.unwrap();

    // remove vesting account
    VESTING_ACCOUNTS.remove(deps.storage, (address.as_str(), &denom));

    let vested_amount = account
        .vesting_schedule
        .vested_amount(env.block.time.seconds())?;
    let claimed_amount = account.claimed_amount;

    // transfer already vested but not claimed amount to
    // a account address or the given `vested_token_recipient` address
    let claimable_amount = vested_amount.checked_sub(claimed_amount)?;
    if !claimable_amount.is_zero() {
        let recipient = vested_token_recipient.unwrap_or_else(|| address.to_string());
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
        let recipient = left_vesting_token_recipient.unwrap_or_else(|| sender.to_string());
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
        ("address", address.as_str()),
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
    use cosmwasm_std::{coins, CosmosMsg, StdError};

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

    // tescase for register_vesting_account with linearvesting
    #[test]
    fn testing_register_vesting_account_with_linear() {
        let env = mock_env();
        let mut deps = mock_dependencies();
        let info = mock_info("sender", &coins(100, DENOM.to_string()));
        let amount: u64 = 100;
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: Some(info.sender.to_string()),
            address: info.sender.clone().into_string(),
            vesting_schedule: VestingSchedule::LinearVesting {
                start_time: 1662824814,
                end_time: 1662824914,
                vesting_amount: Uint128::from(amount),
            },
        };

        let res = execute(deps.as_mut(), env.clone(), info.clone(), msg.clone());
        let deposit_denom = info.funds[0].denom.clone();

        // Amount Should  not equal to zero.
        assert_ne!(
            res,
            Err(StdError::generic_err("assert(vesting_amount > 0)"))
        );

        // Start_time shoul be valid.
        assert_ne!(res, Err(StdError::generic_err("invalid start_time")));

        // End_time Should be valid.
        assert_ne!(res, Err(StdError::generic_err("invalid end_time")));

        // End time should be greater than Start Time.
        assert_ne!(
            res,
            Err(StdError::generic_err("assert(start_time < block_time)"))
        );
        assert_ne!(
            res,
            Err(StdError::generic_err("assert(end_time <= start_time)"))
        );

        // vesting amount and deposit amount should be equal.
        assert_ne!(
            res,
            Err(StdError::generic_err(
                "assert(deposit_amount == vesting_amount)",
            ))
        );

        // Should return Response
        assert_eq!(
            res,
            Ok(Response::new().add_attributes(vec![
                ("action", "register_vesting_account"),
                ("master_address", info.sender.as_str(),),
                ("address", info.sender.as_str()),
                ("vesting_denom", &to_string(&deposit_denom).unwrap()),
                ("vesting_amount", &info.funds[0].amount.to_string()),
            ]))
        )
    }

    // Testcase for Deregistering vesting accounts.
    #[test]
    fn testing_deregister_vesting_account_with_linear() {
        let env = mock_env();
        let mut deps = mock_dependencies();
        let info = mock_info("sender", &coins(100, DENOM.to_string()));
        let amount: u64 = 100;
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: Some(info.sender.to_string()),
            address: info.sender.clone().into_string(),
            vesting_schedule: VestingSchedule::LinearVesting {
                start_time: 1662824814,
                end_time: 1662824914,
                vesting_amount: Uint128::from(amount),
            },
        };

        // Registering the account with linearVesting.
        let _res = execute(deps.as_mut(), env.clone(), info.clone(), msg.clone());
        let deposit_denom = info.funds[0].denom.clone();
        let reciverinfo = mock_info("recipent", &coins(100, DENOM.to_string()));

        // deregister message.
        let msg = ExecuteMsg::DeregisterVestingAccount {
            address: info.sender.clone().into_string(),
            denom: deposit_denom.clone(),
            vested_token_recipient: Some(info.sender.to_string().clone()),
            left_vesting_token_recipient: Some(reciverinfo.sender.to_string()),
        };

        //deregistring account
        let res = execute(deps.as_mut(), env.clone(), info.clone(), msg.clone());
        let messages: Vec<CosmosMsg> = vec![cosmwasm_std::CosmosMsg::Bank(BankMsg::Send {
            to_address: reciverinfo.sender.clone().to_string(),
            amount: vec![Coin {
                denom: "TKN".to_string(),
                amount: Uint128::from(amount),
            }],
        })];

        // Should return response.
        assert_eq!(
            res,
            Ok(Response::new().add_messages(messages).add_attributes(vec![
                ("action", "deregister_vesting_account"),
                ("address", info.sender.as_str()),
                ("vesting_denom", &to_string(&deposit_denom.clone()).unwrap()),
                ("vesting_amount", &100.to_string()),
                ("vested_amount", &0.to_string()),
                ("left_vesting_amount", &100.to_string()),
            ]))
        )
    }

    // Testcase for claim
    #[test]
    fn testing_claim() {
        let env = mock_env();
        let mut deps = mock_dependencies();
        let info = mock_info("sender", &coins(100, DENOM.to_string()));
        let amount: u64 = 100;
        // registering Message
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: Some(info.sender.to_string()),
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
        let reciverinfo = mock_info("recipent", &coins(100, DENOM.to_string()));

        let res = claim(
            deps.as_mut(),
            env,
            info.clone(),
            vec![deposit_denom],
            Some(reciverinfo.sender.clone().into_string()),
        );

        assert_ne!(
            res,
            Err(StdError::generic_err(format!(
                "vesting entry is not found for denom {}",
                to_string(&info.funds[0].denom).unwrap(),
            )))
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

    // testcase for Query to get vesting account
    #[test]
    fn testing_vesting_account() {
        let env = mock_env();
        let mut deps = mock_dependencies();
        let info = mock_info("sender", &coins(100, DENOM.to_string()));
        let amount: u64 = 100;
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: Some(info.sender.to_string()),
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

    // testcase for query to get vesting Tokens.
    #[test]
    fn testing_vesting_tokens() {
        let env = mock_env();
        let mut deps = mock_dependencies();
        let info = mock_info("sender", &coins(100, DENOM.to_string()));
        let amount: u64 = 100;
        // register Message
        let msg = ExecuteMsg::RegisterVestingAccount {
            master_address: Some(info.sender.to_string()),
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
