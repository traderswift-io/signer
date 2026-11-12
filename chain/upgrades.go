package main

import (
	"context"

	"github.com/spf13/cast"

	"github.com/cosmos/cosmos-sdk/client/flags"
	"github.com/cosmos/cosmos-sdk/server"
	servertypes "github.com/cosmos/cosmos-sdk/server/types"
	storetypes "github.com/cosmos/cosmos-sdk/store/v2/types"
	sdk "github.com/cosmos/cosmos-sdk/types"
	"github.com/cosmos/cosmos-sdk/types/module"
	upgradetypes "github.com/cosmos/cosmos-sdk/x/upgrade/types"
)

// Chains that started before the upgrade module existed get its store at one agreed height,
// reached by a halt-height binary swap. Every chain started with this binary has it from genesis.
var upgradeModuleBootstrapHeights = map[string]int64{}

var upgradeNamesWithNoStoreChanges = []string{}

func (app *SignerApp) registerUpgradeHandlers() {
	for _, name := range upgradeNamesWithNoStoreChanges {
		app.upgradeKeeper.SetUpgradeHandler(name, func(ctx context.Context, _ upgradetypes.Plan, fromVersions module.VersionMap) (module.VersionMap, error) {
			return app.moduleManager.RunMigrations(ctx, app.configurator, fromVersions)
		})
	}
}

func (app *SignerApp) setStoreLoaderForUpgradeModuleBootstrap() {
	if height := upgradeModuleBootstrapHeights[app.ChainID()]; height > 0 {
		app.SetStoreLoader(upgradetypes.UpgradeStoreLoader(height, &storetypes.StoreUpgrades{Added: []string{upgradetypes.StoreKey}}))
	}
}

func (app *SignerApp) recordModuleVersionsAtUpgradeModuleBootstrap(ctx sdk.Context) error {
	if height := upgradeModuleBootstrapHeights[ctx.ChainID()]; height > 0 && ctx.BlockHeight() == height {
		return app.upgradeKeeper.SetModuleVersionMap(ctx, app.moduleManager.GetVersionMap())
	}
	return nil
}

func skipUpgradeHeightsFrom(applicationOptions servertypes.AppOptions) map[int64]bool {
	skipHeights := map[int64]bool{}
	for _, height := range cast.ToIntSlice(applicationOptions.Get(server.FlagUnsafeSkipUpgrades)) {
		skipHeights[int64(height)] = true
	}
	return skipHeights
}

func homeDirectoryFrom(applicationOptions servertypes.AppOptions) string {
	return cast.ToString(applicationOptions.Get(flags.FlagHome))
}
