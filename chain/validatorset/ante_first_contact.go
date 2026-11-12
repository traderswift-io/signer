package validatorset

import (
	"context"
	"crypto/sha256"
	"encoding/binary"
	"errors"

	"cosmossdk.io/collections"
	errorsmod "cosmossdk.io/errors"

	sdk "github.com/cosmos/cosmos-sdk/types"
	txsigning "github.com/cosmos/cosmos-sdk/types/tx/signing"
	authkeeper "github.com/cosmos/cosmos-sdk/x/auth/keeper"
)

const MaximumAccountsCreatedPerBlock = 20

const firstContactAccountNumberDomain = "signer first contact account number "

type ChainAccountCreator interface {
	HasAccount(ctx context.Context, address sdk.AccAddress) bool
	NewAccountWithAddress(ctx context.Context, address sdk.AccAddress) sdk.AccountI
	SetAccount(ctx context.Context, account sdk.AccountI)
	HasAccountNumber(ctx context.Context, accountNumber uint64) (bool, error)
}

type AuthChainAccounts struct {
	authkeeper.AccountKeeper
}

func (accounts AuthChainAccounts) HasAccountNumber(ctx context.Context, accountNumber uint64) (bool, error) {
	_, err := accounts.Accounts.Indexes.Number.MatchExact(ctx, accountNumber)
	if errors.Is(err, collections.ErrNotFound) {
		return false, nil
	}
	return err == nil, err
}

type firstContactTx interface {
	sdk.Tx
	GetSigners() ([][]byte, error)
	GetSignaturesV2() ([]txsigning.SignatureV2, error)
}

type FirstContactDecorator struct {
	keeper              *Keeper
	chainAccountCreator ChainAccountCreator
}

func NewFirstContactDecorator(keeper *Keeper, chainAccountCreator ChainAccountCreator) FirstContactDecorator {
	return FirstContactDecorator{keeper: keeper, chainAccountCreator: chainAccountCreator}
}

func FirstContactAccountNumber(address sdk.AccAddress) uint64 {
	digest := sha256.Sum256(append([]byte(firstContactAccountNumberDomain), address...))
	return binary.BigEndian.Uint64(digest[:8]) | uint64(1)<<63
}

func onlyRequestJoinMessages(messages []sdk.Msg) bool {
	if len(messages) == 0 {
		return false
	}
	for _, message := range messages {
		if _, isRequestJoin := message.(*MsgRequestJoin); !isRequestJoin {
			return false
		}
	}
	return true
}

func (keeper *Keeper) accountsCreatedInBlock(ctx sdk.Context) (uint64, error) {
	createdAt, err := keeper.AccountsCreatedAt.Get(ctx)
	if errors.Is(err, collections.ErrNotFound) {
		return 0, nil
	}
	if err != nil {
		return 0, err
	}
	if createdAt.K1() != ctx.BlockHeight() {
		return 0, nil
	}
	return createdAt.K2(), nil
}

// The whole decorator is a pass-through until open join activates, so blocks before that height
// replay with the same gas and the same state as under the binary that produced them.
func (decorator FirstContactDecorator) AnteHandle(ctx sdk.Context, tx sdk.Tx, simulate bool, next sdk.AnteHandler) (sdk.Context, error) {
	active, _, err := decorator.keeper.openJoinIsActive(ctx)
	if err != nil {
		return ctx, err
	}
	signedTx, ok := tx.(firstContactTx)
	if !active || !ok {
		return next(ctx, tx, simulate)
	}
	signers, err := signedTx.GetSigners()
	if err != nil {
		return ctx, err
	}
	signatures, err := signedTx.GetSignaturesV2()
	if err != nil {
		return ctx, err
	}
	unknownSigners := []sdk.AccAddress{}
	for signerIndex, signer := range signers {
		address := sdk.AccAddress(signer)
		if decorator.chainAccountCreator.HasAccount(ctx, address) {
			continue
		}
		if !onlyRequestJoinMessages(tx.GetMsgs()) {
			return ctx, errorsmod.Wrapf(ErrUnauthorized, "%s is not a chain account; its first transaction may hold only RequestJoin", address)
		}
		if signerIndex >= len(signatures) || signatures[signerIndex].Sequence != 0 {
			return ctx, errorsmod.Wrapf(ErrUnauthorized, "%s is not a chain account; its first transaction must have sequence 0", address)
		}
		unknownSigners = append(unknownSigners, address)
	}
	if len(unknownSigners) == 0 {
		return next(ctx, tx, simulate)
	}
	for _, message := range tx.GetMsgs() {
		if err := VerifyRequestJoinProof(ctx.ChainID(), message.(*MsgRequestJoin)); err != nil {
			return ctx, err
		}
	}
	createdInBlock, err := decorator.keeper.accountsCreatedInBlock(ctx)
	if err != nil {
		return ctx, err
	}
	if createdInBlock+uint64(len(unknownSigners)) > MaximumAccountsCreatedPerBlock {
		return ctx, errorsmod.Wrapf(ErrAccountCap, "%d new accounts already created in block %d; resend in the next block", createdInBlock, ctx.BlockHeight())
	}
	for _, address := range unknownSigners {
		taken, err := decorator.chainAccountCreator.HasAccountNumber(ctx, FirstContactAccountNumber(address))
		if err != nil {
			return ctx, err
		}
		if taken {
			return ctx, errorsmod.Wrapf(ErrNumberTaken, "account number %d of %s is held by another account", FirstContactAccountNumber(address), address)
		}
	}
	for _, address := range unknownSigners {
		account := decorator.chainAccountCreator.NewAccountWithAddress(ctx, address)
		if err := account.SetAccountNumber(FirstContactAccountNumber(address)); err != nil {
			return ctx, err
		}
		decorator.chainAccountCreator.SetAccount(ctx, account)
	}
	if err := decorator.keeper.AccountsCreatedAt.Set(ctx, collections.Join(ctx.BlockHeight(), createdInBlock+uint64(len(unknownSigners)))); err != nil {
		return ctx, err
	}
	return next(ctx, tx, simulate)
}
