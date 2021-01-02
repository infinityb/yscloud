with import <nixpkgs> { };

rustPlatform.buildRustPackage rec {
  name = "yscloud-${version}";
  version = "0.2.0";
  src = ./yscloud.tar.gz;
  nativeBuildInputs =  [ pkgconfig openssl protobuf rustfmt git libseccomp ];
  buildInputs = [ pkgconfig openssl protobuf rustfmt git libseccomp ];

  checkPhase = "";
  cargoSha256 = "unset";
  cargoVendorDir = "vendor";

  # for pros and tonic
  PROTOC="${protobuf}/bin/protoc";

  meta = with stdenv.lib; {
    description = "yscloud runtime environment";
    license = licenses.unfree;
    maintainers = [ "Stacey Ell <software@e.staceyell.com>" ];
    platforms = platforms.pc;
  };
}
