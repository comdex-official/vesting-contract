# Code Review

## Contract.rs

**RegisterVestingAccount**

- No addr validation for `address` in RegisterVestingAccount.
- RegisterVestingAccount has a check in LinearVesting that `deposit_amount == vesting_amount`. This `vesting_amount` is an unnecessary check. Isn't it better to vest the `deposit_amount` directly?
- Aren't the two equation below same?
- Master address is not optional, but the comment in *msg.rs* hints that it should be optional.
- Err messages of the type `assert(..)` should either show the correct assertion that will pass, or the actual assertion that failed. Both are being used at the moment which might be confusing to the user if shown, as is.

```rs
time_period != (time_period/vesting_interval)*vesting_interval,
(time_period)%vesting_interval == 0
```
