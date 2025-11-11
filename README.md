i dont know what i was upto but to set it up just run


``cargo build`` to build the dependencies

and to run a peer do ``cargo run --release -- start -p 4001`` port 4001 is optional, use any port

you must need to run 2 or more peers to send files or chat. so for example run another peer on port 4002

to chat just chat your message and press enter, it will be broadcasted across all the peers, and to send a file do ``send filename.txt peerid``


to find peer id you can usually find it at the top of the console when you run the first peer, make sure to send it to the other peer and not to yourself lol

so for example my 2nd peer's ID is ``12D3KooWKASDy3Q7HPB11mnFX5EuiToQDTD1mkm7jpEandnautzp`` i need to do ``send test.txt 12D3KooWKASDy3Q7HPB11mnFX5EuiToQDTD1mkm7jpEandnautzp`` and it will send the file over the network and download it in the /downloads directory


thats all fr
