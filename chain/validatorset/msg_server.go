package validatorset

import (
	"context"
	"strconv"

	errorsmod "cosmossdk.io/errors"

	sdk "github.com/cosmos/cosmos-sdk/types"
	upgradetypes "github.com/cosmos/cosmos-sdk/x/upgrade/types"
)

type msgServer struct {
	keeper *Keeper
}

var _ MsgServer = msgServer{}

func NewMsgServer(keeper *Keeper) MsgServer {
	return msgServer{keeper: keeper}
}

func (server msgServer) AddValidator(ctx context.Context, message *MsgAddValidator) (*MsgAddValidatorResponse, error) {
	if err := server.keeper.requireAdmin(ctx, message.Admin); err != nil {
		return nil, err
	}
	validator := Validator{ConsensusPublicKey: message.ConsensusPublicKey, Power: message.Power, Moniker: message.Moniker, Founding: message.Founding}
	if err := validateValidator(validator); err != nil {
		return nil, err
	}
	alreadyPresent, err := server.keeper.Validators.Has(ctx, message.ConsensusPublicKey)
	if err != nil {
		return nil, err
	}
	if alreadyPresent {
		return nil, errorsmod.Wrapf(ErrAlreadyExists, "validator %s", message.ConsensusPublicKey)
	}
	if err := server.keeper.applyChange(ctx, message.ConsensusPublicKey, &validator); err != nil {
		return nil, err
	}
	emitValidatorSetEvent(ctx, "validator_added",
		sdk.NewAttribute("consensus_public_key", validator.ConsensusPublicKey),
		sdk.NewAttribute("moniker", validator.Moniker),
		sdk.NewAttribute("power", strconv.FormatInt(validator.Power, 10)),
		sdk.NewAttribute("founding", strconv.FormatBool(validator.Founding)))
	return &MsgAddValidatorResponse{}, nil
}

func (server msgServer) SetValidatorPower(ctx context.Context, message *MsgSetValidatorPower) (*MsgSetValidatorPowerResponse, error) {
	if err := server.keeper.requireAdmin(ctx, message.Admin); err != nil {
		return nil, err
	}
	if err := validatePower(message.Power); err != nil {
		return nil, err
	}
	validator, err := server.keeper.Validators.Get(ctx, message.ConsensusPublicKey)
	if err != nil {
		return nil, errorsmod.Wrapf(ErrNotFound, "validator %s", message.ConsensusPublicKey)
	}
	validator.Power = message.Power
	if err := server.keeper.applyChange(ctx, message.ConsensusPublicKey, &validator); err != nil {
		return nil, err
	}
	emitValidatorSetEvent(ctx, "validator_power_set",
		sdk.NewAttribute("consensus_public_key", validator.ConsensusPublicKey),
		sdk.NewAttribute("power", strconv.FormatInt(validator.Power, 10)))
	return &MsgSetValidatorPowerResponse{}, nil
}

func (server msgServer) RemoveValidator(ctx context.Context, message *MsgRemoveValidator) (*MsgRemoveValidatorResponse, error) {
	if err := server.keeper.requireAdmin(ctx, message.Admin); err != nil {
		return nil, err
	}
	present, err := server.keeper.Validators.Has(ctx, message.ConsensusPublicKey)
	if err != nil {
		return nil, err
	}
	if !present {
		return nil, errorsmod.Wrapf(ErrNotFound, "validator %s", message.ConsensusPublicKey)
	}
	if err := server.keeper.applyChange(ctx, message.ConsensusPublicKey, nil); err != nil {
		return nil, err
	}
	if err := server.keeper.forgetJoinerIfPresent(ctx, message.ConsensusPublicKey); err != nil {
		return nil, err
	}
	emitValidatorSetEvent(ctx, "validator_removed", sdk.NewAttribute("consensus_public_key", message.ConsensusPublicKey))
	return &MsgRemoveValidatorResponse{}, nil
}

func (server msgServer) RequestJoin(ctx context.Context, message *MsgRequestJoin) (*MsgRequestJoinResponse, error) {
	sdkCtx := sdk.UnwrapSDKContext(ctx)
	if err := VerifyRequestJoinProof(sdkCtx.ChainID(), message); err != nil {
		return nil, err
	}
	publicKeyBytes, err := decodeConsensusPublicKey(message.ConsensusPublicKey)
	if err != nil {
		return nil, err
	}
	active, params, err := server.keeper.openJoinIsActive(ctx)
	if err != nil {
		return nil, err
	}
	refusal := "open_join_not_active_on_this_chain"
	if active {
		if refusal, err = server.keeper.refusalForRequest(ctx, message, params); err != nil {
			return nil, err
		}
	}
	if refusal == "" {
		if refusal, err = server.keeper.acceptRequest(ctx, message, publicKeyBytes, params); err != nil {
			return nil, err
		}
	}
	if refusal != "" {
		emitValidatorSetEvent(ctx, "join_request_refused",
			sdk.NewAttribute("requester", message.Requester),
			sdk.NewAttribute("consensus_public_key", message.ConsensusPublicKey),
			sdk.NewAttribute("moniker", message.Moniker),
			sdk.NewAttribute("reason", refusal))
		return &MsgRequestJoinResponse{Accepted: false, RefusalReason: refusal}, nil
	}
	emitValidatorSetEvent(ctx, "join_request_accepted",
		sdk.NewAttribute("requester", message.Requester),
		sdk.NewAttribute("consensus_public_key", message.ConsensusPublicKey),
		sdk.NewAttribute("node_id", message.NodeId),
		sdk.NewAttribute("moniker", message.Moniker),
		sdk.NewAttribute("endpoint_ip", message.EndpointIp))
	emitValidatorSetEvent(ctx, "validator_added",
		sdk.NewAttribute("consensus_public_key", message.ConsensusPublicKey),
		sdk.NewAttribute("moniker", message.Moniker),
		sdk.NewAttribute("power", strconv.FormatInt(params.JoinerPower, 10)),
		sdk.NewAttribute("founding", "false"),
		sdk.NewAttribute("owner_address", message.Requester))
	return &MsgRequestJoinResponse{Accepted: true, Power: params.JoinerPower}, nil
}

func (server msgServer) SetOutsiderCountCap(ctx context.Context, message *MsgSetOutsiderCountCap) (*MsgSetOutsiderCountCapResponse, error) {
	if err := server.keeper.requireAdmin(ctx, message.Admin); err != nil {
		return nil, err
	}
	params, err := server.keeper.openJoinParams(ctx)
	if err != nil {
		return nil, err
	}
	params.OutsiderCountCap = message.OutsiderCountCap
	if err := validateOpenJoinParams(params); err != nil {
		return nil, err
	}
	if err := server.keeper.StoredOpenJoinParams.Set(ctx, params); err != nil {
		return nil, err
	}
	emitValidatorSetEvent(ctx, "outsider_count_cap_set", sdk.NewAttribute("outsider_count_cap", strconv.FormatUint(uint64(params.OutsiderCountCap), 10)))
	return &MsgSetOutsiderCountCapResponse{}, nil
}

func (server msgServer) ScheduleUpgrade(ctx context.Context, message *MsgScheduleUpgrade) (*MsgScheduleUpgradeResponse, error) {
	if err := server.keeper.requireAdmin(ctx, message.Admin); err != nil {
		return nil, err
	}
	if server.keeper.upgradeScheduler == nil {
		return nil, errorsmod.Wrap(ErrUnauthorized, "this binary has no upgrade module")
	}
	if err := validateUpgradeInfo(message.Info); err != nil {
		return nil, err
	}
	if err := server.keeper.upgradeScheduler.ScheduleUpgrade(ctx, upgradetypes.Plan{Name: message.Name, Height: message.Height, Info: message.Info}); err != nil {
		return nil, err
	}
	emitValidatorSetEvent(ctx, "upgrade_scheduled",
		sdk.NewAttribute("name", message.Name),
		sdk.NewAttribute("height", strconv.FormatInt(message.Height, 10)),
		sdk.NewAttribute("info", message.Info))
	return &MsgScheduleUpgradeResponse{}, nil
}

func (server msgServer) CancelUpgrade(ctx context.Context, message *MsgCancelUpgrade) (*MsgCancelUpgradeResponse, error) {
	if err := server.keeper.requireAdmin(ctx, message.Admin); err != nil {
		return nil, err
	}
	if server.keeper.upgradeScheduler == nil {
		return nil, errorsmod.Wrap(ErrUnauthorized, "this binary has no upgrade module")
	}
	if err := server.keeper.upgradeScheduler.ClearUpgradePlan(ctx); err != nil {
		return nil, err
	}
	emitValidatorSetEvent(ctx, "upgrade_cancelled")
	return &MsgCancelUpgradeResponse{}, nil
}
