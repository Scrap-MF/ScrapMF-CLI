# Maintainer: Scrap-MF
# Contributor: scrapmf contributors
#
# Build from source. For the prebuilt binary see scrapmf-bin.
#
# After bumping pkgver, refresh checksums with:
#   updpkgsums          # or:
#   makepkg -g >> PKGBUILD
pkgname=scrapmf
pkgver=1.0.0
pkgrel=1
pkgdesc="Safe, interactive archiver for social media galleries"
arch=('x86_64')
url="https://github.com/Scrap-MF/ScrapMF-CLI"
license=('GPL3')
depends=('gcc-libs')
optdepends=('gallery-dl: primary backend'
            'yt-dlp: reserved for future use (videos backend)')
makedepends=('cargo' 'git')
source=("$pkgname-$pkgver.tar.gz::https://github.com/Scrap-MF/ScrapMF-CLI/archive/v$pkgver.tar.gz")
sha256sums=('SKIP')  # replace via updpkgsums before publishing to the AUR

# Run tests during build? Off by default: packaging shouldn't run 109 tests.
if [ -n "$ENABLE_CHECK" ]; then
  check() {
    cd "$pkgname-$pkgver"
    cargo test --all --locked
  }
fi

build() {
  cd "$pkgname-$pkgver"
  cargo build --release --locked
}

package() {
  cd "$pkgname-$pkgver"
  install -Dm755 "target/release/scrapmf" "$pkgdir/usr/bin/scrapmf"
  install -Dm644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
  install -Dm644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE" 2>/dev/null || true
}
