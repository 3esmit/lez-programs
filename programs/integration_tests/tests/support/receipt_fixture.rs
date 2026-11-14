use integration_tests::{account, deploy_at, key, submit};
use lee::{error::LeeError, V03State};
use lee_core::{
    account::{AccountId, Actor, ActorState},
    program::TransactionEvent,
};
use token_comparison_example::receipts::{self, Command, Config, Kind, Request, State};
use token_core::{Message, TokenState};
pub const TOKEN: AccountId = AccountId::new([0xa0; 32]);
pub const PROGRAM: AccountId = AccountId::new([0xb0; 32]);
pub fn id(byte: u8) -> AccountId {
    account(&key(byte))
}
pub fn token(account: AccountId) -> Actor {
    Actor::new(account, TOKEN)
}
pub fn processor() -> Actor {
    Actor::new(id(4), PROGRAM)
}
pub fn escrow(operation: u64) -> AccountId {
    receipts::escrow(processor(), operation)
}
pub fn vault(asset: u8) -> AccountId {
    receipts::vault(processor(), id(asset))
}
pub struct Chain {
    pub state: V03State,
    pub complex: bool,
}
impl Chain {
    pub fn new(complex: bool) -> Self {
        let elf = if complex {
            token_comparison_example::COMPARE_REDEMPTION_ELF
        } else {
            token_comparison_example::COMPARE_PAYMENT_ELF
        };
        let mut state = deploy_at(&[(TOKEN, token_methods::TOKEN_ELF), (PROGRAM, elf)]);
        let assets = if complex {
            vec![(11, 12, 10_000), (13, 14, 20_000), (15, 2, 1_000)]
        } else {
            vec![(1, 2, 1_000)]
        };
        for (definition, holder, supply) in assets {
            submit(
                &mut state,
                token(id(definition)),
                &[token(id(definition)), token(id(holder))],
                &[&key(definition), &key(holder)],
                &Message::NewDefinition {
                    name: format!("Asset {definition}"),
                    total_supply: supply,
                    mint_authority: None,
                    holding: id(holder),
                },
            )
            .expect("asset");
        }
        let recipients = if complex {
            vec![(21, 11), (22, 13)]
        } else {
            vec![(3, 1)]
        };
        for (holder, definition) in recipients {
            submit(
                &mut state,
                token(id(holder)),
                &[token(id(holder)), token(id(definition))],
                &[&key(holder)],
                &Message::InitializeHolding {
                    definition: id(definition),
                },
            )
            .expect("recipient");
        }
        let mut chain = Self { state, complex };
        let kind = if complex {
            Kind::Redemption {
                asset_a: id(11),
                asset_b: id(13),
                shares: id(15),
            }
        } else {
            Kind::Payment {
                definition: id(1),
                beneficiary: id(3),
            }
        };
        let mut actors = chain.actors(0);
        if complex {
            actors.extend([token(id(11)), token(id(13))]);
        }
        submit(
            &mut chain.state,
            processor(),
            &actors,
            &[&key(4)],
            &Command::Initialize(Config {
                token_program: TOKEN,
                kind,
            }),
        )
        .expect("processor");
        if complex {
            chain.donate(11, 12, 1_000);
            chain.donate(13, 14, 2_000);
        }
        chain
    }

    pub fn actors(&self, operation: u64) -> Vec<Actor> {
        let mut actors = vec![
            processor(),
            token(id(2)),
            token(escrow(operation)),
            Actor::new(id(2), PROGRAM),
        ];
        if self.complex {
            actors.extend([
                token(id(15)),
                token(vault(11)),
                token(vault(13)),
                token(id(21)),
                token(id(22)),
            ]);
        } else {
            actors.extend([token(id(1)), token(id(3))]);
        }
        actors
    }

    pub fn fund(
        &mut self,
        operation: u64,
        request: Request,
        signed: bool,
    ) -> Result<Vec<TransactionEvent>, LeeError> {
        self.fund_before(operation, request, signed, 1_000)
    }

    pub fn fund_before(
        &mut self,
        operation: u64,
        request: Request,
        signed: bool,
        expires_before: u64,
    ) -> Result<Vec<TransactionEvent>, LeeError> {
        let mut actors = self.actors(operation);
        actors.push(token(request.source()));
        let user = key(2);
        let signers = if signed { vec![&user] } else { Vec::new() };
        submit(
            &mut self.state,
            processor(),
            &actors,
            &signers,
            &Command::Fund {
                id: operation,
                expires_before,
                request,
            },
        )
    }

    pub fn execute(&mut self, operation: u64) -> Result<Vec<TransactionEvent>, LeeError> {
        let actors = self.actors(operation);
        submit(
            &mut self.state,
            processor(),
            &actors,
            &[],
            &Command::Execute { id: operation },
        )
    }

    pub fn cancel(
        &mut self,
        operation: u64,
        owner: u8,
        signed: bool,
    ) -> Result<Vec<TransactionEvent>, LeeError> {
        let router = Actor::new(id(owner), PROGRAM);
        let mut actors = self.actors(operation);
        actors.push(router);
        let signer = key(owner);
        let signers = if signed { vec![&signer] } else { Vec::new() };
        submit(
            &mut self.state,
            router,
            &actors,
            &signers,
            &Command::Cancel {
                processor: processor().account_id,
                id: operation,
            },
        )
    }

    pub fn data(&self, actor: Actor) -> ActorState {
        self.state
            .get_account_by_id(actor.account_id)
            .data
            .actor_state(actor.program_account_id)
            .clone()
    }

    pub fn snapshot(&self, operation: u64) -> Vec<ActorState> {
        self.actors(operation)
            .into_iter()
            .map(|a| self.data(a))
            .collect()
    }

    pub fn receipt_state(&self) -> State {
        borsh::from_slice(&self.data(processor())).expect("state")
    }

    pub fn balance(&self, account: AccountId) -> u128 {
        let Some(TokenState::Holding(holding)) = TokenState::decode(&self.data(token(account)))
        else {
            panic!("holding")
        };
        holding.balance
    }

    pub fn supply(&self) -> u128 {
        let Some(TokenState::Definition(definition)) =
            TokenState::decode(&self.data(token(id(if self.complex { 15 } else { 1 }))))
        else {
            panic!("definition")
        };
        definition.total_supply
    }

    pub fn donate(&mut self, asset: u8, holder: u8, amount: u128) {
        submit(
            &mut self.state,
            token(id(holder)),
            &[token(id(holder)), token(vault(asset))],
            &[&key(holder)],
            &Message::Transfer {
                to: vault(asset),
                amount,
            },
        )
        .expect("donation");
    }
}

pub fn submit_at(
    chain: &mut Chain,
    operation: u64,
    message: &Command,
    now: u64,
) -> Result<Vec<TransactionEvent>, LeeError> {
    use lee::{
        public_transaction::{Message as TransactionMessage, WitnessSet},
        PublicTransaction,
    };
    let tx = TransactionMessage::try_new(
        processor(),
        chain.actors(operation),
        Default::default(),
        message,
    )
    .expect("transaction");
    let witnesses = WitnessSet::for_message(&tx, &[]);
    chain
        .state
        .transition_from_public_transaction(&PublicTransaction::new(tx, witnesses), 1, now)
}
