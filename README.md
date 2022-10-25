# Vesting Contract

This contract provides vesting account feature for cw20 and native tokens.

## Execute Operations

* RegisterVestingAccount - Register a new vesting account

  ```rust
    RegisterVestingAccount {
        master_address: Option<String>,
        address: String,
        vesting_schedule: VestingSchedule,
    }
  ```

  * When creating a new vesting account, the user needs to specify a master address (`master_address`), which enables deregister feature. The recipient of the vesting amount is specified using the `address` parameter. One of two vesting schedules is specified using `vesting_schedule`.
  * For a given `address` and token (OSMO, CMDX, etc) pair, only a single vesting account is allowed.

* DeregisterVestingAccount - Deregister vesting account

  ```rust
    DeregisterVestingAccount {
        denom: Denom,
        vested_token_recipient: Option<String>,
    }
  ```

  * During a deregister operation, there is a possibility that the account has unclaimed vested amount (`claimable_amount`) and any remaining vesting amount (`left_vesting_amount`). The unclaimed vested amount is transferred to the recipient specified in the msg parameters (`vested_token_recipient`) or to the original recipient for whom the account was created in the first place.
  * Amount which is yet to be vested is transferred to the master address.

* Claim - Claim vested (unlocked) tokens.

  ```rust
    Claim {
        denoms: Vec<Denom>,
        recipient: Option<String>,
    }
  ```

  * Allows a user to claim vested tokens for the given denom(s) (`denoms`).
  * The vested tokens may be optionally sent to another recipient specified through the `recipient` parameter.

**NOTE:** Amount which can be claimed by the user, that is the unlocked amount in accordance with the *vesting schedule*, is referred to as the *vested* amount. Amount which is yet to be unlocked is referred to as *vesting* amount.

## Query Operations

* VestingAccount - Query current vesting accounts present for the given address.

  ```rust
    VestingAccount {
        address: String,
        start_after: Option<Denom>,
        limit: Option<u32>,
    },
  ```

  * For a given user (`address`), query the vesting accounts.
  * This query also implements pagination, given by the optional `start_after` and `limit` parameters. The former represents the starting point of pagination and the latter specifies the number of tokens to include in the reponse.
  * Response of the above query includes the user (`address`) for whom the above query was run and the vesting data for each token. Refer to [VestingAccountResponse](#query-responses) for more details.

  **NOTE:** The default limit is set to **10** and the maximum limit is set to **30**.

* VestedTokens - Query amount of vested tokens for the given denomination.

  ```rust
    VestedTokens {
        denom: String,
    },

  ```

  * Quries the contract for vesting account details of a single denomination (`denom`) associated with the sender.
  * Response contains the total amount of vested tokens.

### Query Responses

* VestingAccountResponse - Response type of the *VestingAccount* query.

  ```rust
    pub struct VestingAccountResponse {
        pub address: String,
        pub vestings: Vec<VestingData>,
    }
  ```

  * `address` represents the user for whom the contract was queried.
  * `vestings` consists of an array of vesting details for each queried token.

  * VestingData - Struct that holds the vesting details.

    ```rust
      pub struct VestingData {
          pub master_address: String,
          pub vesting_denom: String,
          pub vesting_amount: Uint128,
          pub vested_amount: Uint128,
          pub vesting_schedule: VestingSchedule,
          pub claimable_amount: Uint128,
      }
    ```

    * `master_address` - master address for the vesting tokens. If the vesting account is deregistered prior to all tokens being vested, then the remaining vesting tokens are transferred to the master address.
    * `vesting_denom` - denomination of the vesting tokens.
    * `vesting_amount` - amount of tokens that were deposited for vesting.
    * `vested_amount` - amount that has already vested and may be claimed.
    * `vesting_schedule` - the schedule of the vesting tokens.
    * `claimable_amount` - amount of tokens which may be claimed.

### Deployed Contract Info

**NEED TO UPDATE THIS**

| data          | bombay-12                                    | columbus-5 |
| ------------- | -------------------------------------------- | ---------- |
| code_id       | 35340                                        | N/A        |
| contract_addr | terra15uc49grd8h0xxj3jvmcx9yswvw8v0ypy32pe8m | N/A        |
