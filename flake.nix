{
  description = "wadachi (轍) — directory frecency: shared store CLI + ashiato-niwa background indexer";

  # substrate.rust.workspace (gen path). The module trio the fleet consumes was
  # dropped by the bare gen conversion (aec25c0); this restores the exact
  # pre-conversion module spec (extraHmOptions already in lib:-function form, so
  # no nixpkgs input needed — substrate threads lib in).
  inputs.substrate.url = "github:pleme-io/substrate";

  outputs = { substrate, ... }: substrate.rust.workspace {
    src = ./.;
    member = "pleme-io-wadachi";
      module = {
        description = "wadachi (轍) directory frecency — shared store CLI + ashiato-niwa background indexer";
        hmNamespace = "services";
        withUserDaemon = true;
        userDaemonSubcommand = "indexd";
        extraHmOptions = lib: {
          indexer = {
            enable = lib.mkOption {
              type = lib.types.bool;
              default = false;
              description = ''
                Run the ashiato-niwa background directory indexer
                (`wadachi indexd`) as a user-level launchd agent (Darwin) /
                systemd user unit (Linux). Sets
                services.wadachi.daemon.enable by default — but the
                module-trio gates ALL config (this alias included) on
                services.wadachi.enable, so BOTH switches are required:
                  services.wadachi.enable = true;
                  services.wadachi.indexer.enable = true;
                Use services.wadachi.daemon.{extraArgs,environment} for
                knobs (e.g. WADACHI_TIER / WADACHI_DB).
              '';
            };
          };
        };
        extraHmConfigFn = { cfg, lib, ... }: {
          services.wadachi.daemon.enable = lib.mkDefault (cfg.indexer.enable or false);
        };
      };
  };
}
