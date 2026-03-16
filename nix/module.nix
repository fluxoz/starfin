# NixOS module for running Starfin as a systemd service.
#
# Example usage in a NixOS configuration (flake-based):
#
#   {
#     inputs.starfin.url = "github:fluxoz/starfin";
#
#     outputs = { nixpkgs, starfin, ... }: {
#       nixosConfigurations.my-host = nixpkgs.lib.nixosSystem {
#         modules = [
#           starfin.nixosModules.default
#           {
#             services.starfin = {
#               enable = true;
#               videoLibraryPath = "/mnt/videos";
#               bindAddr = "0.0.0.0";
#               openFirewall = true;
#               theme = "nord";          # or "catppuccin", "dracula", "jetson"
#               design = "editorial";    # or "neubrutalist", "aero"
#               # themeFile = ./my-theme.toml;   # fully custom TOML theme
#             };
#           }
#         ];
#       };
#     };
#   }
{ self }:
{ config, lib, pkgs, ... }:
let
  inherit (lib) mkEnableOption mkOption mkIf types literalExpression getExe;
  cfg = config.services.starfin;
in
{
  options.services.starfin = {
    enable = mkEnableOption "the Starfin media server";

    package = mkOption {
      type = types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      defaultText = literalExpression "starfin.packages.\${system}.default";
      description = "The starfin package to use.";
    };

    port = mkOption {
      type = types.port;
      default = 8089;
      example = 8080;
      description = ''
        TCP port that Starfin listens on (`PORT` environment variable).
      '';
    };

    bindAddr = mkOption {
      type = types.str;
      default = "127.0.0.1";
      example = "0.0.0.0";
      description = ''
        Address to bind to (`BIND_ADDR` environment variable).
        Set to `0.0.0.0` to expose Starfin on all network interfaces.
      '';
    };

    videoLibraryPath = mkOption {
      type = types.path;
      example = "/mnt/videos";
      description = ''
        Directory that Starfin scans for video files (`VIDEO_LIBRARY_PATH`
        environment variable).  This option is required.
      '';
    };

    cacheDir = mkOption {
      type = types.path;
      default = "/var/cache/starfin";
      description = ''
        Directory used to store HLS segments and thumbnail cache (`CACHE_DIR`
        environment variable).
      '';
    };

    openFirewall = mkOption {
      type = types.bool;
      default = false;
      description = "Open the firewall for the configured `port`.";
    };

    passwordProtection = mkOption {
      type = types.bool;
      default = false;
      description = ''
        Enable password protection.  When enabled, a password modal gates
        access to the library.  The hashed password is stored in the cache
        directory (`PASSWORD_PROTECTION` environment variable).
      '';
    };

    theme = mkOption {
      type = types.enum [ "jetson" "nord" "catppuccin" "dracula" ];
      default = "jetson";
      example = "nord";
      description = ''
        UI color theme.  Starfin ships with four built-in presets:

        - **jetson** – warm beige with orange accents (default)
        - **nord** – Arctic, cool-blue toned (Nord color scheme)
        - **catppuccin** – soothing pastels (Catppuccin Latte / Mocha)
        - **dracula** – purple / pink dark-first palette

        Set to a preset name, or use `themeFile` for a fully custom TOML
        theme (`THEME` environment variable).
      '';
    };

    design = mkOption {
      type = types.enum [ "editorial" "neubrutalist" "aero" ];
      default = "editorial";
      example = "neubrutalist";
      description = ''
        UX design (style language).  Controls typography, geometry, and
        visual effects independently of the color theme.

        - **editorial** – monospace, uppercase, thick borders (default)
        - **neubrutalist** – sans-serif, zero radius, hard shadows
        - **aero** – glass morphism, rounded, translucent (Y2K aesthetic)

        Designs are composable with any color theme
        (`DESIGN` environment variable).
      '';
    };

    themeFile = mkOption {
      type = types.nullOr types.path;
      default = null;
      example = literalExpression "./my-theme.toml";
      description = ''
        Path to a custom TOML theme file.  When set this takes precedence
        over the `theme` option.  See `themes/example.toml` in the Starfin
        repository for the file format (`THEME_FILE` environment variable).
      '';
    };

    user = mkOption {
      type = types.str;
      default = "starfin";
      description = "System user account under which the Starfin process runs.";
    };

    group = mkOption {
      type = types.str;
      default = "starfin";
      description = "System group under which the Starfin process runs.";
    };

    extraEnvironment = mkOption {
      type = types.attrsOf types.str;
      default = { };
      example = literalExpression ''
        {
          RUST_LOG = "info";
        }
      '';
      description = ''
        Additional environment variables passed verbatim to the Starfin process.
        These take precedence over any module-managed variables.
      '';
    };
  };

  config = mkIf cfg.enable {
    # ── Users & groups ────────────────────────────────────────────────────────
    users.users = mkIf (cfg.user == "starfin") {
      starfin = {
        isSystemUser = true;
        group = cfg.group;
        description = "Starfin media server";
      };
    };

    users.groups = mkIf (cfg.group == "starfin") {
      starfin = { };
    };

    # ── Firewall ──────────────────────────────────────────────────────────────
    networking.firewall.allowedTCPPorts = mkIf cfg.openFirewall [ cfg.port ];

    # ── Persistent directories ────────────────────────────────────────────────
    systemd.tmpfiles.rules = [
      "d '${cfg.cacheDir}' 0750 ${cfg.user} ${cfg.group} - -"
    ];

    # ── Systemd service ───────────────────────────────────────────────────────
    systemd.services.starfin = {
      description = "Starfin Media Server";
      wantedBy = [ "multi-user.target" ];
      after = [ "network.target" ];

      path = [ pkgs.ffmpeg ];

      environment = {
        PORT = toString cfg.port;
        BIND_ADDR = cfg.bindAddr;
        VIDEO_LIBRARY_PATH = toString cfg.videoLibraryPath;
        CACHE_DIR = toString cfg.cacheDir;
        THEME = cfg.theme;
        DESIGN = cfg.design;
      } // (lib.optionalAttrs cfg.passwordProtection {
        PASSWORD_PROTECTION = "true";
      }) // (lib.optionalAttrs (cfg.themeFile != null) {
        THEME_FILE = toString cfg.themeFile;
      }) // cfg.extraEnvironment;

      serviceConfig = {
        ExecStart = getExe cfg.package;
        User = cfg.user;
        Group = cfg.group;
        Restart = "on-failure";
        RestartSec = "5s";

        # Security hardening
        NoNewPrivileges = true;
        PrivateTmp = true;
        ProtectSystem = "strict";
        ProtectHome = true;
        ReadWritePaths = [
          cfg.cacheDir
          (toString cfg.videoLibraryPath)
        ];
      };
    };
  };
}
