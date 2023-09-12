use anyhow::{anyhow, Result};
use bytes::Bytes;
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

use revm::{
    db::{CacheDB, EmptyDB, InMemoryDB},
    EVM,
};

#[derive(Debug, Clone)]
pub struct TxResult {
    pub output: Bytes,
    pub logs: Option<Vec<Log>>,
    pub gas_used: u64,
    pub gas_refunded: u64,

}

pub fn get_token_balance(evm: &mut EVM<InMemoryDB>, token: H160, account: H1)

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
