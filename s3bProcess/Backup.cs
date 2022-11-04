using System;
using System.Collections.Generic;
using System.Text;
using System.IO;
using System.Reflection;
using System.Data.Common;

namespace s3b
{
    public class Backup : Job
    {

         PersistBase persist = null;

         string bucket = string.Empty;
         string backup_folder = string.Empty;
            int maxRetry = 3;

        public Backup()
        {

        }

        override public bool run(Model args)
        {
            int retry = 0;
            persist = PersistBase.Persistence;

            Logger.Persist = persist;

            Logger.info("starting...");

            persist.execCmd("delete from message_log");

            BackupSet bset = BackupSet.factory(args);

            buildWorkingSet(bset);

            do
            {
                process(bset);
            }
            while (!recon(bset) && (retry++ < maxRetry));

            bset.last_backup_datetime = DateTime.Now;

            persist.update(bset);

            Logger.info("complete.");

            return true;
        }

        private void buildWorkingSet(BackupSet bset)
        {
            load(bset);

            folders(bset);

            files(bset);

            recon(bset);
        }


        bool recon(BackupSet bset)
        {
            bool result = true;
            if (!isStepEnabled("recon")) return result;

            Logger.info("reconciling...");

            Dictionary<string, LocalFolder> uploadedFolders = bset.getUploadedFolders();

            ProcExec.OutputCallback stdout = (parts) =>
            {
                ObjectInfo objectInfo = ObjectInfo.factory(parts);

                if (uploadedFolders.ContainsKey(objectInfo.encrypted_file_name))
                {
                    LocalFolder fldr = uploadedFolders[objectInfo.encrypted_file_name];

                    Logger.info("checking " + objectInfo.encrypted_file_name);

                    if (objectInfo.encrypted_file_size == fldr.encrypted_file_size)
                    {
                        Logger.info("encrypted file size ok");
                    }
                    else
                    {
                        Logger.error(fldr.folder_path + " size does not match uploaded " + objectInfo.encrypted_file_name);
                        result = false;
                        addWorkingFolder(bset, fldr);
                    }
                }
                else
                {
                    Logger.info(objectInfo.encrypted_file_name + " not found. skipping.");
                }

            };

            ProcExec.OutputCallback stderr = (parts) =>
            {
                Logger.error(parts);

            };

            exec("recon.command", "recon.args", stdout, stderr);

            return result;
        }


        private  void setParameters(LocalFolder fldr)
        {
            Config config = Config.getConfig();

            config.setValue("localfolder", fldr.folder_path);
            config.setValue("localfile", fldr.folder_path + "\\*");
            config.setValue("archive.name", fldr.getArchiveName());
        }

        

         void load(BackupSet bset)
        {
            Logger.info("loading backup set " + bset.root_folder_path);

            persist.put(bset, "root_folder_path");
        }

         void folders(BackupSet bset)
        {
            string[] dirs = Directory.GetDirectories(bset.root_folder_path);

            Logger.info("loading folders for backup set " + bset.root_folder_path);

            LocalFolder fldr;

            foreach (string d in dirs)
            {
                childFolderFactory(bset, d);
            }

            fldr = rootFolderFactory(bset);
        }

        private LocalFolder childFolderFactory(BackupSet bset, string d)
        {
            
            return folderFactory(bset, d, true);
        }

        private LocalFolder rootFolderFactory(BackupSet bset)
        {
            

            return folderFactory(bset, bset.root_folder_path, false);
        }

        private LocalFolder folderFactory( BackupSet bset, string folderPath, bool recurse )
        {
            LocalFolder fldr = new LocalFolder();
            fldr.backup_set_id = bset.id;
            fldr.folder_path = folderPath;
            fldr.recurse = recurse;
            fldr.backupSet = bset;
            persist.put(fldr, "folder_path");
            persist.get(fldr);
            fldr.status = "new";
            fldr.status = "none";

            bset.localFolders.Add(fldr.id, fldr);
            fldr.backupSet = bset;
            return fldr;
        }

        delegate void FileCallback(string filename);

        // put all files into the database recursively
         void files(BackupSet bset)
        {
            foreach (LocalFolder fldr in bset.localFolders.Values)
            {
                try
                {

                    Logger.info("loading files for folder " + fldr.folder_path);
                    bool filesChanged = false;
                

                    FileCallback dcb = (filename) =>
                    {
                        FileInfo fi = new FileInfo(filename);

                        
                        if (fi.LastWriteTime > fldr.upload_datetime)
                        {
                            filesChanged = true;
                            fldr.backupLogs.Add(BackupLog.factory(Config.getConfig(), fldr, fi));
                        }
                    };

               

                    if (fldr.recurse) dirSearch(fldr.folder_path, dcb);

                
                    foreach (string f in Directory.GetFiles(fldr.folder_path))
                    {
                        dcb(f);
                    }

                    if (filesChanged)
                    {
                        addWorkingFolder(bset, fldr);
                    }
                }
                catch (Exception x)
                {
                    Logger.error(x);
                }
            }
        }

        private  void dirSearch(string folder_path, FileCallback dcb)
        {
            try
            {
                foreach (string d in Directory.GetDirectories(folder_path))
                {

                    dirSearch(d, dcb);
                }

                foreach (string f in Directory.GetFiles(folder_path))
                {
                    dcb(f);
                }
            }
            catch (Exception x)
            {
                Logger.error(x);
            }
        }

        private void addWorkingFolder(BackupSet bset, LocalFolder fldr)
        {
            Logger.info("adding working folder: " + fldr.folder_path);
            if (!bset.workFolders.ContainsKey(fldr.folder_path))
                bset.workFolders.Add(fldr.folder_path, fldr);
            updateStatus(fldr, "new", "none");
        }

        

        void process(BackupSet bset)
        {

            foreach (LocalFolder fldr in bset.workFolders.Values)
            {
                
                setParameters(fldr);

                int stageCode = fldr.getStageCode();

                Logger.info("processing: " + fldr.folder_path);
                if ((stageCode & (int)LocalFolder.stages.archiveStage) == (int)LocalFolder.stages.archiveStage)
                    archive(fldr);

                if ((stageCode & (int)LocalFolder.stages.compressStage) == (int)LocalFolder.stages.compressStage)
                    compress(fldr);

                if ((stageCode & (int)LocalFolder.stages.encryptStage) == (int)LocalFolder.stages.encryptStage)
                    encrypt(fldr);


                if ((stageCode & (int)LocalFolder.stages.uploadStage) == (int)LocalFolder.stages.uploadStage)
                    upload(fldr);

                if ((stageCode & (int)LocalFolder.stages.cleanStage) == (int)LocalFolder.stages.cleanStage)
                    clean(fldr);

                writeBackupLog(fldr);

            }
        }

        private  void archive(LocalFolder fldr)
        {
            Config config = Config.getConfig();

            if (!isStepEnabled("archive")) return;

            Logger.info("archiving: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");

            int retCode = 0;


            if (fldr.recurse)
            {
                config.setValue("localobject", fldr.folder_path);
                retCode = exec("archive.command", "archive.args");
            }
            else
            {
                //foreach (LocalFile f in fldr.files)
                string[] files = Directory.GetFiles(fldr.folder_path);
                foreach (string f in files)
                {                    
                    config.setValue("localfile", f);
                    config.setValue("localobject", f);

                    int execCode = exec("archive.command", "archive.args");
                    if (execCode == 1) retCode = 1;
                }
            }


            string completion = "complete";
            if (retCode != 0) completion = "error";

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
        }



         void compress(LocalFolder fldr)
        {
            if (!isStepEnabled("compress")) return;

            Logger.info("compressing: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");

            int retCode = exec("compress.command", "compress.args");
            string completion = "complete";
            if (retCode != 0) completion = "error";



            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);

        }

         void encrypt(LocalFolder fldr)
        {
            Config config = Config.getConfig();

            if (!isStepEnabled("encrypt")) return;

            Logger.info("encrypting: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");

            

            int retCode = exec("encrypt.command", "encrypt.args");
            string completion = "complete";
            if (retCode == 0)
            {
                FileInfo fi = new FileInfo(config.getString("encrypt.clean"));

                fldr.encrypted_file_name = config.getString("encrypt.target");
                fldr.encrypted_file_size = fi.Length;

                persist.update(fldr);
            }
            else
            {
                completion = "error";
            }

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
        }

         void upload(LocalFolder fldr)
        {
            Config config = Config.getConfig();

            if (!isStepEnabled("upload")) return;

            Logger.info("uploading: " + fldr.folder_path);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, "in_progess");


            config.setValue("uploadtarget", fldr.backupSet.upload_target);

            string completion = "complete";
            if (exec("upload.command", "upload.args") != 0) completion = "error";

            fldr.upload_datetime = DateTime.Now;
            persist.update(fldr);

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);
        }


         

         
        

         void clean(LocalFolder fldr)
        {
            Logger.info("cleaning temp files: " + fldr.folder_path);


            //  clean("archive.clean", cmdParams); // not necessary as gzip removes source fuke


            clean("compress.clean");
            clean("encrypt.clean");


            string completion = "complete";

            updateStatus(fldr, MethodBase.GetCurrentMethod().Name, completion);

        }

         void clean(string configName)
        {
            Config config = Config.getConfig();

            string filename = config.getString(configName);

            if (File.Exists(filename))
            {
                Logger.info("cleaning " + filename);
                File.Delete(filename);
            }
            else
            {
                Logger.info("unable to clean " + filename);
            }

        }

        void writeBackupLog(LocalFolder fldr)
        {
            foreach(BackupLog b in fldr.backupLogs)
            {
                b.last_upload_time = fldr.upload_datetime;
                persist.insert(b);
            }
        }

         void updateStatus(LocalFolder fldr, string stage, string status)
        {
            fldr.stage = stage;
            fldr.status = status;
            persist.update(fldr);
        }

         bool isStepEnabled(string stepName)
        {
            bool result = false;

            int enabled = Config.getConfig().getInt(stepName + ".enabled");

            if (enabled == 1) result = true;

            return result;
        }
    }
}
