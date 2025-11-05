1. If a multivalue (email/phone) field is selected. On Enter press: open modal dialog that lists all values as a table: [value; type]; tab/backspace & up/down selects the row; q/Escape closes the modal; 'd' sets the selected value as default (and moves the corresponding entry to be thje first phone/email entry in the vcard). space copies the value for a selected row and quits the modal; when the modal is open, diisplay help line explaining hotkeys in the status bar
2. Search:
Currently: if search is on, search field auto active; select in search results with up/down; enter closes search;
Needed: 
    - Search key ('/' by default) opens search, focuses on search bar;
    - Escape closes search bar;
    - Enter focuses on search result list; 
    - When focused on search result list:
        * search key refocuses on search bar
        * up/down moves current selection; tab/backspace moves the current selection; 
        * the card display on the right (main card; tabs & image pane) update to display current selection; 
        * Enter closes search; 
        * Escape closes search;
4. Do not display N* fields for ORGs
5. When editing a field, status bar should say: "EDITING $FIELD. ESCAPE TO CANCEL".
6. F1 opens a modal with all hot keys
7. When focused on search result list:
  - Space toggles "mark" on the contact
  - Shift+Space toggles between showing search results & list of marked contacts
8. Pressing 'm' merges marked contacts; Merge strategy for N vcards:
  - The result of the merge is that a new vcard is created; old vcards are removed;
  - Merge inductively; First merge the first 2 vcards following the described strategy, then merge the resulting vcard with the 3d vcard etc. Therefore, the merge strategy is described for a pair of vcards only, but is applicable to an arbitrary amount
  - Read ./merge-strategy.md for strategy
