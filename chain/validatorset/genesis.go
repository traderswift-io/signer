package validatorset

import (
	"context"
	"errors"

	abci "github.com/cometbft/cometbft/abci/types"

	"cosmossdk.io/collections"
	errorsmod "cosmossdk.io/errors"

	sdk "github.com/cosmos/cosmos-sdk/types"
)

func DefaultGenesisState() *GenesisState {
	return &GenesisState{}
}

func ValidateGenesisState(state *GenesisState) error {
	if _, err := sdk.AccAddressFromBech32(state.Admin); err != nil {
		return errorsmod.Wrapf(ErrInvalidField, "admin %q: %v", state.Admin, err)
	}
	if state.PendingAdmin != "" {
		if _, err := sdk.AccAddressFromBech32(state.PendingAdmin); err != nil {
			return errorsmod.Wrapf(ErrInvalidField, "pending_admin %q: %v", state.PendingAdmin, err)
		}
		if state.PendingAdmin == state.Admin {
			return errorsmod.Wrap(ErrInvalidField, "pending_admin is already the admin")
		}
	}
	seenKeys := map[string]bool{}
	validators := make([]Validator, 0, len(state.Validators))
	for _, validator := range state.Validators {
		if validator == nil {
			return errorsmod.Wrap(ErrInvalidField, "empty validator entry")
		}
		if err := validateValidator(*validator); err != nil {
			return err
		}
		if seenKeys[validator.ConsensusPublicKey] {
			return errorsmod.Wrapf(ErrAlreadyExists, "validator %s twice in genesis", validator.ConsensusPublicKey)
		}
		seenKeys[validator.ConsensusPublicKey] = true
		validators = append(validators, *validator)
	}
	if state.OpenJoinParams != nil {
		if err := validateOpenJoinParams(*state.OpenJoinParams); err != nil {
			return err
		}
	}
	for _, joiner := range state.Joiners {
		if joiner == nil || !seenKeys[joiner.ConsensusPublicKey] {
			return errorsmod.Wrap(ErrInvalidField, "every joiner must be a validator in genesis")
		}
	}
	return requireSetWithinOutsiderLimit(validators)
}

func (keeper *Keeper) InitializeFromGenesis(ctx context.Context, state *GenesisState) ([]abci.ValidatorUpdate, error) {
	if err := ValidateGenesisState(state); err != nil {
		return nil, err
	}
	if err := keeper.AdminAddress.Set(ctx, state.Admin); err != nil {
		return nil, err
	}
	if state.PendingAdmin != "" {
		if err := keeper.PendingAdminAddress.Set(ctx, state.PendingAdmin); err != nil {
			return nil, err
		}
	}
	updates := []abci.ValidatorUpdate{}
	for _, validator := range state.Validators {
		if err := keeper.Validators.Set(ctx, validator.ConsensusPublicKey, *validator); err != nil {
			return nil, err
		}
		if err := keeper.PowerLastSentToConsensus.Set(ctx, validator.ConsensusPublicKey, validator.Power); err != nil {
			return nil, err
		}
		update, err := consensusUpdate(validator.ConsensusPublicKey, validator.Power)
		if err != nil {
			return nil, err
		}
		updates = append(updates, update)
	}
	if state.OpenJoinParams != nil {
		if err := keeper.StoredOpenJoinParams.Set(ctx, *state.OpenJoinParams); err != nil {
			return nil, err
		}
	}
	for _, joiner := range state.Joiners {
		publicKeyBytes, err := decodeConsensusPublicKey(joiner.ConsensusPublicKey)
		if err != nil {
			return nil, err
		}
		if err := keeper.Joiners.Set(ctx, joiner.ConsensusPublicKey, *joiner); err != nil {
			return nil, err
		}
		if err := keeper.JoinerKeyByOwner.Set(ctx, joiner.OwnerAddress, joiner.ConsensusPublicKey); err != nil {
			return nil, err
		}
		if err := keeper.JoinerKeyByEndpointIp.Set(ctx, joiner.EndpointIp, joiner.ConsensusPublicKey); err != nil {
			return nil, err
		}
		if err := keeper.JoinerKeyByConsensusAddress.Set(ctx, consensusAddressOf(publicKeyBytes), joiner.ConsensusPublicKey); err != nil {
			return nil, err
		}
	}
	for _, cooldown := range state.RejoinCooldowns {
		if err := keeper.RejoinCooldownUntil.Set(ctx, cooldown.OwnerAddress, cooldown.UntilUnixSeconds); err != nil {
			return nil, err
		}
	}
	if err := keeper.ValidatorSetChangedThisBlock.Set(ctx, false); err != nil {
		return nil, err
	}
	return updates, nil
}

func (keeper *Keeper) ExportToGenesis(ctx context.Context) (*GenesisState, error) {
	admin, err := keeper.AdminAddress.Get(ctx)
	if err != nil {
		return nil, err
	}
	validators, err := keeper.allValidators(ctx)
	if err != nil {
		return nil, err
	}
	pendingAdmin, err := keeper.pendingAdmin(ctx)
	if err != nil {
		return nil, err
	}
	state := &GenesisState{Admin: admin, PendingAdmin: pendingAdmin}
	for index := range validators {
		state.Validators = append(state.Validators, &validators[index])
	}
	params, err := keeper.StoredOpenJoinParams.Get(ctx)
	if err == nil {
		state.OpenJoinParams = &params
	} else if !errors.Is(err, collections.ErrNotFound) {
		return nil, err
	}
	joinerIterator, err := keeper.Joiners.Iterate(ctx, nil)
	if err != nil {
		return nil, err
	}
	defer joinerIterator.Close()
	for ; joinerIterator.Valid(); joinerIterator.Next() {
		joiner, err := joinerIterator.Value()
		if err != nil {
			return nil, err
		}
		state.Joiners = append(state.Joiners, &joiner)
	}
	cooldownIterator, err := keeper.RejoinCooldownUntil.Iterate(ctx, nil)
	if err != nil {
		return nil, err
	}
	defer cooldownIterator.Close()
	for ; cooldownIterator.Valid(); cooldownIterator.Next() {
		entry, err := cooldownIterator.KeyValue()
		if err != nil {
			return nil, err
		}
		state.RejoinCooldowns = append(state.RejoinCooldowns, &RejoinCooldown{OwnerAddress: entry.Key, UntilUnixSeconds: entry.Value})
	}
	return state, nil
}
