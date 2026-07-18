# Part Command

## Overview
The `/part` command provides an easy way to search and display information about a specific LEGO part directly within Discord. It automatically queries the Rebrickable API for part details and provides a quick external link to the BrickLink catalog.

## Trigger
- Triggered as `/part` (or `/pala` depending on the `locale` defined in `config.toml`).
- The command takes one **required** argument: `query` (a part number e.g. `3001` or a search term e.g. `Brick 2x4`).

## Features

1. **Part Information Display**:
   - Displays the part's name, production years, print details, molds, and alternates.
   - Embeds the part image thumbnail.

2. **Search Capability**:
   - If the `query` matches multiple parts, the bot will return an ephemeral dropdown menu allowing the user to select the specific part they intended.

3. **External Link Integration**:
   - The bot dynamically generates and displays links to:
     - BrickLink
     - Rebrickable

4. **Service Customization**:
   - Upon running the command, the user is presented with a dropdown menu allowing them to toggle which external services (BrickLink, LEGO.com, etc.) they want visible.
   - The bot persists these preferences per-user in the local SQLite database, ensuring that future queries default to the user's preferred platforms.

## Configuration
The command can be enabled or disabled globally or on a per-guild basis using the `config.toml` file:

```toml
[commands.part]
enabled = true

[[guilds]]
guild_id = 123456789
[guilds.commands.part]
enabled = false # Disables /part for this specific guild
```

## Appendix: Understanding LEGO Part Numbering Systems

Because `brickbot` relies on APIs like Rebrickable and integrates with sites like BrickLink, understanding how different platforms identify parts is crucial. The ecosystem uses a variety of overlapping numbering systems.

### 1. The Official LEGO Numbers
The LEGO Group utilizes two primary identifiers internally and on their services:

*   **Design ID:** This is a 4- or 5-digit number (e.g., `3001`) that identifies the **shape** of a part, regardless of color. It is often molded directly onto the underside of the physical brick. This is the most universal number across all platforms.
*   **Element ID:** This is a 6- or 7-digit number that identifies a specific part in a specific **color** (and print). It represents a unique variation of a Design ID. These numbers are printed in the back of official instruction manuals and are used for customer service orders.

### 2. Community and Marketplace Systems
Third-party platforms have evolved independently, leading to variations in how they catalog parts, especially printed or modified ones.

*   **BrickLink:** Designed primarily for the secondary marketplace, BrickLink generally uses the official Design ID as a base but heavily modifies it to track highly specific inventory variations. 
    *   They often use suffixes for mold variations (e.g., `3245b`, `3245c`).
    *   Printed or stickered parts receive a unique alphanumeric suffix (e.g., `3068pb001`), which is entirely different from LEGO's official Element IDs.
*   **Rebrickable:** Focused on the MOC (My Own Creation) community and inventory management, Rebrickable acts as a database bridge.
    *   They maintain their own database, heavily leaning on official Design IDs, but they also maintain extensive mapping tables connecting their numbers to both BrickLink's catalog numbers and official LEGO Element IDs.
    *   Rebrickable often groups minor mold variations together (if they don't affect building compatibility) but allows you to differentiate if necessary.
*   **Brickset:** Primarily a set database rather than a parts marketplace, Brickset usually relies on official LEGO Element IDs and Design IDs when listing set inventories, pulling data directly from LEGO where possible.

### Why the Differences Exist?
The fragmentation exists due to differing goals:
- **LEGO** needs to track manufacturing variations and colors (Element IDs).
- **BrickLink** needs hyper-specific tracking for collectors and sellers to distinguish between old and new molds or minor print variations.
- **Rebrickable** needs functional equivalence to help users figure out if they can build a custom model with the parts they already own.

**When using the `/part` command:** The bot uses the **Rebrickable API**, which accepts the core **Design ID** (or Rebrickable's specific variation IDs). The bot then attempts to map this Rebrickable data to generate external links to BrickLink using external ID mappings provided by Rebrickable.

### 3. Examples and API Integration
To illustrate how the same physical piece is represented across the ecosystem, let's look at the classic **2x4 Brick in Bright Red**:

| Service / API | Type of ID | Example ID | Example Display Name | Example Web URL / API Endpoint |
| :--- | :--- | :--- | :--- | :--- |
| **Rebrickable** | Design ID | `3001` | `Brick 2 x 4` | [Web Part Page](https://rebrickable.com/parts/3001/)<br/>API Endpoint (`https://rebrickable.com/api/v3/lego/parts/3001/`) |
| **BrickLink** | Item No | `3001` | `Brick 2 x 4` | [Web Catalog (`?P=3001`)](https://www.bricklink.com/v2/catalog/catalogitem.page?P=3001)<br/>[API Endpoint (`/api/store/v1/items/part/3001`)](https://api.bricklink.com/api/store/v1/items/part/3001) |
| **Brickset** | Design ID / Element ID | `3001` | `BRICK 2X4` | Web Parts Catalog (`https://brickset.com/parts/design-3001`)<br/>*(Note: Brickset API focuses on sets, not parts)* |

#### Printed Part Example (Tile 2x2 with Newspaper Print)
*   **BrickLink:** `3068bpb001` (Name: `Tile 2 x 2 with Newspaper 'The LEGO News' Pattern`) - Uses Shape `3068b` + Print Identifier `001`.
*   **Rebrickable:** `3068bpr0004` (Name: `Tile 2 x 2 with Newspaper 'THE LEGO NEWS' Print`) - Uses a similar but slightly different suffix system mapped to BrickLink.
