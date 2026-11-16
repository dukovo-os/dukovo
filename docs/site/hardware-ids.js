/*
 * SPDX-FileCopyrightText: 2026 The Dukovo contributors
 * SPDX-License-Identifier: Apache-2.0 OR MIT
 *
 * The PCI IDs that "Check your machine" in the Supported hardware chapter
 * knows: each row is [driver, vendor, device IDs], the driver being a
 * folder of kernel/drivers. A test of `ob` checks that every ID here is in
 * that driver's source, so a row cannot claim a card the driver does not
 * take. A driver that learns a card adds its ID here in the same change.
 */
window.HW_IDS = {
  // Ethernet controllers (PCI class 0200).
  network: [
    ['intel', '8086', '100e 100f 1010 1013 1014 1015 1016 1017 1018 1019 101a 101d 101e 1026 105e 1075 1076 1077 1078 1079 107c 107d 108b 108c 1099 109a 10a4 10b5 10b9 10bc 10d3 10d5 10f6 150c'],
    ['intel', '8086', '10c9 10e8 150a 150e 1516 1521 1526 1533 1534 1535 1539 157b'],
    ['intel', '8086', '0d9f 125b 125c 125d 125e 15f2 15f3 15f7 15f8 3100 3101 3102 5502 5503'],
    ['intel', '8086', '10f7 10fb 1528 154d 1557 1560 1563 15d1'],
    ['intel', '8086', '0d53 0d55 1502 1503 156f 1570 15b7 15b8 15b9 15d6 15d7 15d8 15e3'],
    ['realtek', '10ec', '8169 8167 8168 8161 8136 2600'],
    ['realtek', '1186', '4300 4302'],
    ['realtek', '10ff', '8168'],
    ['realtek', '1259', 'c107'],
    ['realtek', '1737', '1032'],
    ['realtek', '16ec', '0116'],
    ['rge', '10ec', '8125 8126 3000 5000'],
    ['atheros', '1969', '1062 1063 1073 1083 2060 2062 1090 1091 10a0 10a1 e091 e0a1 e0b1'],
    ['broadcom', '14e4', '1655 1665 1656 1657 165f 1680 1681 1687 1682 1686 16b0 16b1 16b3 16b4 16b5 16b7 1692'],
    ['virtio', '1af4', '1000 1041'],
    ['vmxnet3', '15ad', '07b0'],
    ['ena', '1d0f', 'ec20 ec21 0ec2 1ec2'],
    ['gve', '1ae0', '0042'],
    ['mana', '1414', '00ba']
  ],
  // Intel VMD domains, whose disks the kernel reaches (class 0104).
  vmd: [['vmd', '8086', '201d 28c0 9a0b 4c3d 467f a77f 7d0b ad0b b06f b60b']],
  // Realtek's card readers, which have no standard class.
  cardReader: [['rtsx', '10ec', '5209 5227 5229 522a 5249 525a 5286 5287 5289']]
};
