package main

import (
	"encoding/json"
	"fmt"

	abci "github.com/cometbft/cometbft/abci/types"
	dbm "github.com/cosmos/cosmos-db"
	"github.com/cosmos/gogoproto/proto"

	autocliv1 "cosmossdk.io/api/cosmos/autocli/v1"
	"cosmossdk.io/client/v2/autocli"
	"cosmossdk.io/core/appmodule"
	"cosmossdk.io/log/v2"

	"github.com/cosmos/cosmos-sdk/baseapp"
	"github.com/cosmos/cosmos-sdk/client"
	"github.com/cosmos/cosmos-sdk/client/grpc/cmtservice"
	nodeservice "github.com/cosmos/cosmos-sdk/client/grpc/node"
	"github.com/cosmos/cosmos-sdk/codec"
	"github.com/cosmos/cosmos-sdk/codec/address"
	codectypes "github.com/cosmos/cosmos-sdk/codec/types"
	"github.com/cosmos/cosmos-sdk/runtime"
	runtimeservices "github.com/cosmos/cosmos-sdk/runtime/services"
	"github.com/cosmos/cosmos-sdk/server"
	"github.com/cosmos/cosmos-sdk/server/api"
	"github.com/cosmos/cosmos-sdk/server/config"
	servertypes "github.com/cosmos/cosmos-sdk/server/types"
	"github.com/cosmos/cosmos-sdk/std"
	storetypes "github.com/cosmos/cosmos-sdk/store/v2/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/module"
	"github.com/cosmos/cosmos-sdk/version"
	"github.com/cosmos/cosmos-sdk/x/auth"
	"github.com/cosmos/cosmos-sdk/x/auth/ante"
	authcodec "github.com/cosmos/cosmos-sdk/x/auth/codec"
	authkeeper "github.com/cosmos/cosmos-sdk/x/auth/keeper"
	"github.com/cosmos/cosmos-sdk/x/auth/posthandler"
	authtx "github.com/cosmos/cosmos-sdk/x/auth/tx"
	authtypes "github.com/cosmos/cosmos-sdk/x/auth/types"
	"github.com/cosmos/cosmos-sdk/x/bank"
	bankkeeper "github.com/cosmos/cosmos-sdk/x/bank/keeper"
	banktypes "github.com/cosmos/cosmos-sdk/x/bank/types"
	"github.com/cosmos/cosmos-sdk/x/consensus"
	consensusparamkeeper "github.com/cosmos/cosmos-sdk/x/consensus/keeper"
	consensusparamtypes "github.com/cosmos/cosmos-sdk/x/consensus/types"
	"github.com/cosmos/cosmos-sdk/x/genutil"
	genutiltypes "github.com/cosmos/cosmos-sdk/x/genutil/types"
	"github.com/cosmos/cosmos-sdk/x/tx/signing"
	"github.com/cosmos/cosmos-sdk/x/upgrade"
	upgradekeeper "github.com/cosmos/cosmos-sdk/x/upgrade/keeper"
	upgradetypes "github.com/cosmos/cosmos-sdk/x/upgrade/types"

	"signer/chain/policy"
	"signer/chain/validatorset"
)

const applicationName = "signer"

const bech32AccountPrefix = "signer"

func setBech32Prefixes() {
	configuration := sdk.GetConfig()
	configuration.SetBech32PrefixForAccount(bech32AccountPrefix, bech32AccountPrefix+"pub")
	configuration.SetBech32PrefixForValidator(bech32AccountPrefix+"valoper", bech32AccountPrefix+"valoperpub")
	configuration.SetBech32PrefixForConsensusNode(bech32AccountPrefix+"valcons", bech32AccountPrefix+"valconspub")
	configuration.Seal()
}

var _ servertypes.Application = (*SignerApp)(nil)

type SignerApp struct {
	*baseapp.BaseApp
	legacyAmino        *codec.LegacyAmino
	appCodec           codec.Codec
	txConfig           client.TxConfig
	interfaceRegistry  codectypes.InterfaceRegistry
	accountKeeper      authkeeper.AccountKeeper
	bankKeeper         bankkeeper.BaseKeeper
	consensusKeeper    consensusparamkeeper.Keeper
	upgradeKeeper      *upgradekeeper.Keeper
	validatorSetKeeper *validatorset.Keeper
	policyKeeper       *policy.Keeper
	moduleManager      *module.Manager
	basicModuleManager module.BasicManager
	configurator       module.Configurator
}

func newSignerApp(logger log.Logger, db dbm.DB, loadLatest bool, applicationOptions servertypes.AppOptions, baseAppOptions ...func(*baseapp.BaseApp)) *SignerApp {
	interfaceRegistry, err := codectypes.NewInterfaceRegistryWithOptions(codectypes.InterfaceRegistryOptions{
		ProtoFiles: proto.HybridResolver,
		SigningOptions: signing.Options{
			AddressCodec:          address.Bech32Codec{Bech32Prefix: sdk.GetConfig().GetBech32AccountAddrPrefix()},
			ValidatorAddressCodec: address.Bech32Codec{Bech32Prefix: sdk.GetConfig().GetBech32ValidatorAddrPrefix()},
		},
	})
	if err != nil {
		panic(err)
	}
	appCodec := codec.NewProtoCodec(interfaceRegistry)
	legacyAmino := codec.NewLegacyAmino()
	if err := interfaceRegistry.SigningContext().Validate(); err != nil {
		panic(err)
	}
	std.RegisterLegacyAminoCodec(legacyAmino)
	std.RegisterInterfaces(interfaceRegistry)
	txConfig, err := authtx.NewTxConfigWithOptions(appCodec, authtx.ConfigOptions{})
	if err != nil {
		panic(err)
	}

	baseApp := baseapp.NewBaseApp(applicationName, logger, db, txConfig.TxDecoder(), baseAppOptions...)
	baseApp.SetVersion(version.Version)
	baseApp.SetInterfaceRegistry(interfaceRegistry)
	baseApp.SetTxEncoder(txConfig.TxEncoder())

	storeKeys := storetypes.NewKVStoreKeys(authtypes.StoreKey, banktypes.StoreKey, consensusparamtypes.StoreKey, validatorset.StoreKey, policy.StoreKey, upgradetypes.StoreKey)
	unchangeableAuthority := authtypes.NewModuleAddress(consensusparamtypes.ModuleName).String()
	moduleAccountPermissions := map[string][]string{authtypes.FeeCollectorName: nil}
	blockedAddresses := map[string]bool{authtypes.NewModuleAddress(authtypes.FeeCollectorName).String(): true}

	app := &SignerApp{BaseApp: baseApp, legacyAmino: legacyAmino, appCodec: appCodec, txConfig: txConfig, interfaceRegistry: interfaceRegistry}
	app.consensusKeeper = consensusparamkeeper.NewKeeper(appCodec, runtime.NewKVStoreService(storeKeys[consensusparamtypes.StoreKey]), unchangeableAuthority, runtime.EventService{})
	baseApp.SetParamStore(app.consensusKeeper.ParamsStore)
	app.accountKeeper = authkeeper.NewAccountKeeper(appCodec, runtime.NewKVStoreService(storeKeys[authtypes.StoreKey]), authtypes.ProtoBaseAccount,
		moduleAccountPermissions, authcodec.NewBech32Codec(bech32AccountPrefix), bech32AccountPrefix, unchangeableAuthority)
	app.bankKeeper = bankkeeper.NewBaseKeeper(appCodec, runtime.NewKVStoreService(storeKeys[banktypes.StoreKey]), app.accountKeeper, blockedAddresses, unchangeableAuthority, logger)
	app.validatorSetKeeper = validatorset.NewKeeper(appCodec, runtime.NewKVStoreService(storeKeys[validatorset.StoreKey]))
	app.upgradeKeeper = upgradekeeper.NewKeeper(skipUpgradeHeightsFrom(applicationOptions), runtime.NewKVStoreService(storeKeys[upgradetypes.StoreKey]), appCodec,
		homeDirectoryFrom(applicationOptions), baseApp, authtypes.NewModuleAddress(validatorset.ModuleName).String())
	app.validatorSetKeeper.SetUpgradeScheduler(app.upgradeKeeper)
	app.policyKeeper = policy.NewKeeper(appCodec, runtime.NewKVStoreService(storeKeys[policy.StoreKey]), app.validatorSetKeeper)

	app.moduleManager = module.NewManager(
		genutil.NewAppModule(app.accountKeeper, nil, app, txConfig),
		auth.NewAppModule(appCodec, app.accountKeeper, nil),
		bank.NewAppModule(appCodec, app.bankKeeper, app.accountKeeper),
		consensus.NewAppModule(appCodec, app.consensusKeeper),
		validatorset.NewAppModule(app.validatorSetKeeper),
		policy.NewAppModule(app.policyKeeper),
		upgrade.NewAppModule(app.upgradeKeeper, authcodec.NewBech32Codec(bech32AccountPrefix)),
	)
	app.basicModuleManager = module.NewBasicManagerFromManager(app.moduleManager, map[string]module.AppModuleBasic{
		genutiltypes.ModuleName: genutil.NewAppModuleBasic(genutiltypes.DefaultMessageValidator),
	})
	app.basicModuleManager.RegisterLegacyAminoCodec(legacyAmino)
	app.basicModuleManager.RegisterInterfaces(interfaceRegistry)

	app.moduleManager.SetOrderPreBlockers(upgradetypes.ModuleName, authtypes.ModuleName)
	app.moduleManager.SetOrderBeginBlockers(genutiltypes.ModuleName, validatorset.ModuleName)
	app.moduleManager.SetOrderEndBlockers(banktypes.ModuleName, genutiltypes.ModuleName, validatorset.ModuleName)
	genesisModuleOrder := []string{authtypes.ModuleName, banktypes.ModuleName, genutiltypes.ModuleName, consensusparamtypes.ModuleName, validatorset.ModuleName, policy.ModuleName, upgradetypes.ModuleName}
	app.moduleManager.SetOrderInitGenesis(genesisModuleOrder...)
	app.moduleManager.SetOrderExportGenesis(genesisModuleOrder...)

	app.configurator = module.NewConfigurator(appCodec, app.MsgServiceRouter(), app.GRPCQueryRouter())
	if err := app.moduleManager.RegisterServices(app.configurator); err != nil {
		panic(err)
	}
	autocliv1.RegisterQueryServer(app.GRPCQueryRouter(), runtimeservices.NewAutoCLIQueryService(app.moduleManager.Modules))

	app.MountKVStores(storeKeys)
	app.SetInitChainer(app.initChainer)
	app.registerUpgradeHandlers()
	app.setStoreLoaderForUpgradeModuleBootstrap()
	app.SetPreBlocker(func(ctx sdk.Context, _ *abci.RequestFinalizeBlock) (*sdk.ResponsePreBlock, error) {
		if err := app.recordModuleVersionsAtUpgradeModuleBootstrap(ctx); err != nil {
			return nil, err
		}
		return app.moduleManager.PreBlock(ctx)
	})
	app.SetBeginBlocker(func(ctx sdk.Context) (sdk.BeginBlock, error) { return app.moduleManager.BeginBlock(ctx) })
	app.SetEndBlocker(func(ctx sdk.Context) (sdk.EndBlock, error) { return app.moduleManager.EndBlock(ctx) })

	app.SetAnteHandler(sdk.ChainAnteDecorators(
		ante.NewSetUpContextDecorator(),
		ante.NewExtensionOptionsDecorator(nil),
		ante.NewValidateBasicDecorator(),
		ante.NewTxTimeoutHeightDecorator(),
		ante.NewValidateMemoDecorator(app.accountKeeper),
		ante.NewConsumeGasForTxSizeDecorator(app.accountKeeper),
		validatorset.NewFirstContactDecorator(app.validatorSetKeeper, validatorset.AuthChainAccounts{AccountKeeper: app.accountKeeper}),
		ante.NewDeductFeeDecorator(app.accountKeeper, app.bankKeeper, nil, nil),
		ante.NewSetPubKeyDecorator(app.accountKeeper),
		ante.NewValidateSigCountDecorator(app.accountKeeper),
		ante.NewSigGasConsumeDecorator(app.accountKeeper, ante.DefaultSigVerificationGasConsumer),
		ante.NewSigVerificationDecorator(app.accountKeeper, txConfig.SignModeHandler()),
		ante.NewIncrementSequenceDecorator(app.accountKeeper),
	))
	postHandler, err := posthandler.NewPostHandler(posthandler.HandlerOptions{})
	if err != nil {
		panic(err)
	}
	app.SetPostHandler(postHandler)

	if loadLatest {
		if err := app.LoadLatestVersion(); err != nil {
			panic(fmt.Errorf("loading latest version: %w", err))
		}
	}
	return app
}

func (app *SignerApp) initChainer(ctx sdk.Context, request *abci.RequestInitChain) (*abci.ResponseInitChain, error) {
	var genesisState map[string]json.RawMessage
	if err := json.Unmarshal(request.AppStateBytes, &genesisState); err != nil {
		return nil, err
	}
	if err := app.upgradeKeeper.SetModuleVersionMap(ctx, app.moduleManager.GetVersionMap()); err != nil {
		return nil, err
	}
	return app.moduleManager.InitGenesis(ctx, app.appCodec, genesisState)
}

func (app *SignerApp) autoCliOptions() autocli.AppOptions {
	modules := make(map[string]appmodule.AppModule)
	for _, registeredModule := range app.moduleManager.Modules {
		named, hasName := registeredModule.(module.HasName)
		if !hasName {
			continue
		}
		if appModule, isAppModule := named.(appmodule.AppModule); isAppModule {
			modules[named.Name()] = appModule
		}
	}
	return autocli.AppOptions{
		Modules:               modules,
		ModuleOptions:         runtimeservices.ExtractAutoCLIOptions(app.moduleManager.Modules),
		AddressCodec:          authcodec.NewBech32Codec(sdk.GetConfig().GetBech32AccountAddrPrefix()),
		ValidatorAddressCodec: authcodec.NewBech32Codec(sdk.GetConfig().GetBech32ValidatorAddrPrefix()),
		ConsensusAddressCodec: authcodec.NewBech32Codec(sdk.GetConfig().GetBech32ConsensusAddrPrefix()),
	}
}

func (app *SignerApp) RegisterAPIRoutes(apiServer *api.Server, _ config.APIConfig) {
	clientContext := apiServer.ClientCtx
	authtx.RegisterGRPCGatewayRoutes(clientContext, apiServer.GRPCGatewayRouter)
	cmtservice.RegisterGRPCGatewayRoutes(clientContext, apiServer.GRPCGatewayRouter)
	nodeservice.RegisterGRPCGatewayRoutes(clientContext, apiServer.GRPCGatewayRouter)
	app.basicModuleManager.RegisterGRPCGatewayRoutes(clientContext, apiServer.GRPCGatewayRouter)
	policy.RegisterRestRoutes(clientContext, apiServer.Router)
}

func (app *SignerApp) RegisterTxService(clientContext client.Context) {
	authtx.RegisterTxService(app.GRPCQueryRouter(), clientContext, app.Simulate, app.interfaceRegistry)
}

func (app *SignerApp) RegisterTendermintService(clientContext client.Context) {
	cometApplication := server.NewCometABCIWrapper(app)
	cmtservice.RegisterTendermintService(clientContext, app.GRPCQueryRouter(), app.interfaceRegistry, cometApplication.Query)
}

func (app *SignerApp) RegisterNodeService(clientContext client.Context, serverConfig config.Config) {
	nodeservice.RegisterNodeService(clientContext, app.GRPCQueryRouter(), serverConfig, func() int64 {
		return app.CommitMultiStore().EarliestVersion()
	})
}
