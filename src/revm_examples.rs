use anyhow::{anyhow, Result};
use revm::primitives::bytes::Bytes;
use ethers::{
    abi::{self, parse_abi},
    prelude::*,
    providers::Middleware,
    types::{
        transaction::eip2930::AccessList, BlockId, BlockNumber, Eip1559TransactionRequest,
        NameOrAddress, H160, U256,
    },
};
use log::info;
use revm::{
    db::{CacheDB, EmptyDB, EthersDB, InMemoryDB},
    primitives::Bytecode,
    primitives::{
        keccak256, AccountInfo, ExecutionResult, Log, Output, TransactTo, TxEnv, B160,
        U256 as rU256,
    },
    Database, EVM,
};
use std::{str::FromStr, sync::Arc};

#[derive(Debug, Clone)]
pub struct TxResult {
    pub output: Bytes,
    pub logs: Option<Vec<Log>>,
    pub gas_used: u64,
    pub gas_refunded: u64,
}

pub fn create_evm_instance() -> EVM<InMemoryDB> {
    let db = CacheDB::new(EmptyDB::default());
    let mut evm = EVM::new();
    evm.database(db);
    evm
}

pub fn evm_env_setup(evm: &mut EVM<InMemoryDB>) {
    // overriding some default env values to make it more efficient for testing
    evm.env.cfg.limit_contract_code_size = Some(0x1000000);
    evm.env.cfg.disable_block_gas_limit = true;
    evm.env.cfg.disable_base_fee = true;
}

//We’ll try calling the ERC-20 function “balanceOf” to check how much of the token balance our simulated user holds
pub fn get_token_balance(evm: &mut EVM<InMemoryDB>, token: H160, account: H160) -> Result<U256> {
    
    //First, we create the BaseContract instance from a simple ERC-20 ABI that contains a single function definition:
    let erc20_abi = BaseContract::from(parse_abi(&[
        "function balanceOf(address) external view returs (uint256)",
    ])?);

    //With this, we encode the function call to “balanceOf”, which will result in the below, if we try printing out “calldata”:
    //Bytes(0x70a08231000000000000000000000000e2b5a9c1e325511a227ef527af38c3a7b65afa1d)
    let calldata = erc20_abi.encode("balanceOf", account)?;

    //We, then, have to setup the transaction variables before we can send the call:
    //caller: who is calling the function (from)
    evm.env.tx.caller = account.into();
    //transact_to: what are we calling (to)
    evm.env.tx.transact_to = TransactTo::Call(token.into());
    //data: the input data of our transaction (input)
    evm.env.tx.data = calldata.0;

    let result = match evm.transact_ref() {
        Ok(result) => result,
        Err(e) => return Err(anyhow!("EVM call failed: {e:?}")),
    };

    let tx_result = match result.result {
        ExecutionResult::Success {
            gas_used,
            gas_refunded,
            output,
            logs,
            ..
        } => match output {
            Output::Call(o) => TxResult {
                output: o,
                logs: Some(logs),
                gas_used,
                gas_refunded,
            },
            Output::Create(o, _) => TxResult {
                output: o,
                logs: Some(logs),
                gas_used,
                gas_refunded,
            },
        },
        ExecutionResult::Revert { gas_used, output } => {
            return Err(anyhow!(
                "EVM REVERT: {:?} / Gas used: {:?}",
                output,
                gas_used
            ))
        }
        ExecutionResult::Halt {
            reason, gas_used, ..
        } => return Err(anyhow!("EVM HALT: {:?} / Gas used: {:?}", reason, gas_used)),

    };
    let decoded_output = erc20_abi.decode_output("balanceOf", tx_result.output)?;
    Ok(decoded_output)
}


